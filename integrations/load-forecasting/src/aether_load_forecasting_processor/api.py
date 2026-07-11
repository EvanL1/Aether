"""FastAPI transport for the request-driven processor."""

from __future__ import annotations

import asyncio
import hmac
import os
import re
import threading
from collections.abc import Awaitable, Callable
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass
from typing import Any, TypeVar

from fastapi import APIRouter, FastAPI
from fastapi.responses import JSONResponse
from pydantic import ValidationError
from starlette.requests import Request
from starlette.types import Message, Receive, Scope, Send

from .models import DataProcessingRequest, ProcessingError, ProcessingResult
from .processor import LoadForecastProcessor, ProcessorRequestError

DEFAULT_MAX_REQUEST_BYTES = 1_048_576
MEDIA_TYPE = "application/vnd.aether.data-processing+json;version=1"
BEARER_TOKEN_ENV = "AETHER_LOAD_FORECASTING_BEARER_TOKEN"
REQUIRE_AUTH_ENV = "AETHER_LOAD_FORECASTING_REQUIRE_AUTH"
MAX_CONCURRENCY_ENV = "AETHER_LOAD_FORECASTING_MAX_CONCURRENCY"
_BEARER_TOKEN = re.compile(r"^[A-Za-z0-9._~+/=-]{1,8192}$")

_T = TypeVar("_T")


class DataProcessingJSONResponse(JSONResponse):
    """Versioned JSON response shared with the Rust HTTP adapter."""

    media_type = MEDIA_TYPE


def _error_response(
    *,
    status_code: int,
    code: str,
    category: str,
    message: str,
    retryable: bool,
    request_id: str | None = None,
    headers: dict[str, str] | None = None,
) -> DataProcessingJSONResponse:
    error = ProcessingError(
        request_id=request_id,
        code=code,
        category=category,
        message=message,
        retryable=retryable,
    )
    return DataProcessingJSONResponse(
        status_code=status_code,
        content=error.model_dump(exclude_none=True),
        headers=headers,
    )


def _strict_environment_bool(name: str, *, default: bool) -> bool:
    value = os.getenv(name)
    if value is None:
        return default
    normalized = value.strip().lower()
    if normalized == "true":
        return True
    if normalized == "false":
        return False
    raise ValueError(f"{name} must be either true or false")


@dataclass(frozen=True, slots=True)
class BearerAuthPolicy:
    """Optional bearer authentication with an explicit production fail-closed switch."""

    token: str | None = None
    required: bool = False

    def __post_init__(self) -> None:
        token = self.token
        if token is not None and (len(token) < 32 or not _BEARER_TOKEN.fullmatch(token)):
            raise ValueError("configured bearer token is invalid")
        if self.required and token is None:
            raise ValueError("a bearer token is required but not configured")

    @classmethod
    def from_env(cls) -> BearerAuthPolicy:
        token = os.getenv(BEARER_TOKEN_ENV) or None
        return cls(
            token=token,
            required=_strict_environment_bool(REQUIRE_AUTH_ENV, default=False),
        )

    @property
    def enabled(self) -> bool:
        return self.token is not None

    def authorizes(self, authorization: bytes | None) -> bool:
        if self.token is None:
            return True
        if authorization is None:
            return False
        try:
            header = authorization.decode("ascii")
        except UnicodeDecodeError:
            return False
        scheme, separator, credential = header.partition(" ")
        if not separator or scheme.lower() != "bearer" or not credential or " " in credential:
            return False
        return hmac.compare_digest(credential, self.token)


class BearerAuthMiddleware:
    """Authenticate the processing endpoint before reading its request body."""

    def __init__(self, app: Callable[..., Awaitable[None]], policy: BearerAuthPolicy) -> None:
        self.app = app
        self.policy = policy

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] == "http" and scope.get("path") == "/v1/process" and self.policy.enabled:
            headers = {key.lower(): value for key, value in scope.get("headers", [])}
            if not self.policy.authorizes(headers.get(b"authorization")):
                response = _error_response(
                    status_code=401,
                    code="AUTHENTICATION_REQUIRED",
                    category="authorization",
                    message="valid bearer authentication is required",
                    retryable=False,
                    headers={"WWW-Authenticate": "Bearer"},
                )
                await response(scope, receive, send)
                return
        await self.app(scope, receive, send)


class ProcessorBusy(RuntimeError):
    """All commissioned processor execution slots remain occupied."""


class ProcessorRunner:
    """Bound synchronous model work without releasing slots on HTTP cancellation."""

    def __init__(self, max_concurrency: int = 1) -> None:
        if max_concurrency <= 0 or max_concurrency > 256:
            raise ValueError("max_concurrency must be inside [1, 256]")
        self._slots = threading.BoundedSemaphore(max_concurrency)
        self._executor = ThreadPoolExecutor(
            max_workers=max_concurrency,
            thread_name_prefix="aether-load-forecast",
        )
        self._state_lock = threading.Lock()
        self._closed = False

    async def run(self, function: Callable[..., _T], *args: Any) -> _T:
        if not self._slots.acquire(blocking=False):
            raise ProcessorBusy
        with self._state_lock:
            if self._closed:
                self._slots.release()
                raise RuntimeError("processor runner is closed")
            try:
                future = self._executor.submit(function, *args)
            except Exception:
                self._slots.release()
                raise
        future.add_done_callback(self._release_slot)
        wrapped = asyncio.wrap_future(future)
        try:
            return await asyncio.shield(wrapped)
        except asyncio.CancelledError:
            # `shield` deliberately keeps the model thread alive. Consume a later
            # exception while the concurrent-future callback owns slot release.
            wrapped.add_done_callback(self._consume_background_result)
            raise

    def close(self) -> None:
        with self._state_lock:
            if self._closed:
                return
            self._closed = True
        self._executor.shutdown(wait=False, cancel_futures=False)

    def _release_slot(self, _future: Future[Any]) -> None:
        self._slots.release()

    @staticmethod
    def _consume_background_result(future: asyncio.Future[Any]) -> None:
        if not future.cancelled():
            future.exception()


def _configured_max_concurrency() -> int:
    raw = os.getenv(MAX_CONCURRENCY_ENV, "1")
    try:
        value = int(raw)
    except ValueError as exc:
        raise ValueError(f"{MAX_CONCURRENCY_ENV} must be an integer") from exc
    if value <= 0 or value > 256:
        raise ValueError(f"{MAX_CONCURRENCY_ENV} must be inside [1, 256]")
    return value


class RequestSizeLimitMiddleware:
    """Bound `/v1/process` before JSON parsing, with or without Content-Length."""

    def __init__(self, app: Callable[..., Awaitable[None]], max_bytes: int) -> None:
        if max_bytes <= 0:
            raise ValueError("max_bytes must be positive")
        self.app = app
        self.max_bytes = max_bytes

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or scope.get("path") != "/v1/process":
            await self.app(scope, receive, send)
            return

        headers = {key.lower(): value for key, value in scope.get("headers", [])}
        raw_length = headers.get(b"content-length")
        if raw_length is not None:
            try:
                content_length = int(raw_length)
            except ValueError:
                await _error_response(
                    status_code=400,
                    code="CONTENT_LENGTH_INVALID",
                    category="invalid_request",
                    message="content-length is invalid",
                    retryable=False,
                )(scope, receive, send)
                return
            if content_length < 0 or content_length > self.max_bytes:
                await self._too_large(scope, receive, send)
                return

        body = bytearray()
        while True:
            inbound = await receive()
            if inbound["type"] == "http.disconnect":
                return
            if inbound["type"] != "http.request":
                continue
            body.extend(inbound.get("body", b""))
            if len(body) > self.max_bytes:
                await self._too_large(scope, receive, send)
                return
            if not inbound.get("more_body", False):
                break

        replayed = False

        async def replay() -> Message:
            nonlocal replayed
            if replayed:
                return {"type": "http.disconnect"}
            replayed = True
            return {"type": "http.request", "body": bytes(body), "more_body": False}

        await self.app(scope, replay, send)

    @staticmethod
    async def _too_large(scope: Scope, receive: Receive, send: Send) -> None:
        response = _error_response(
            status_code=413,
            code="FRAME_TOO_LARGE",
            category="resource_limit",
            message="request body exceeds configured limit",
            retryable=False,
        )
        await response(scope, receive, send)


def create_router(
    processor: LoadForecastProcessor,
    *,
    runner: ProcessorRunner | None = None,
) -> APIRouter:
    router = APIRouter()
    processor_runner = runner or ProcessorRunner()

    @router.post(
        "/v1/process",
        response_model=ProcessingResult,
        response_model_exclude_none=True,
        response_class=DataProcessingJSONResponse,
    )
    async def process(request: Request) -> DataProcessingJSONResponse:
        content_type = request.headers.get("content-type", "").lower()
        if (
            content_type != MEDIA_TYPE
            and content_type.split(";", 1)[0].strip() != "application/json"
        ):
            return _error_response(
                status_code=415,
                code="MEDIA_TYPE_UNSUPPORTED",
                category="invalid_request",
                message="request content type is not supported",
                retryable=False,
            )
        try:
            typed_request = DataProcessingRequest.model_validate_json(await request.body())
            result = await processor_runner.run(processor.process, typed_request)
            return DataProcessingJSONResponse(
                content=result.model_dump(mode="json", by_alias=True, exclude_none=True)
            )
        except ProcessorBusy:
            return _error_response(
                status_code=429,
                code="PROCESSOR_BUSY",
                category="capacity",
                message="processor concurrency limit is occupied",
                retryable=True,
                request_id=(typed_request.request_id if "typed_request" in locals() else None),
                headers={"Retry-After": "1"},
            )
        except ProcessorRequestError as exc:
            return _error_response(
                status_code=exc.status_code,
                code=exc.code,
                category=exc.category,
                message=exc.public_message,
                retryable=exc.retryable,
                request_id=exc.request_id,
            )
        except ValidationError:
            return _error_response(
                status_code=422,
                code="FRAME_INVALID",
                category="invalid_data",
                message="request does not satisfy the Data Processing v1 contract",
                retryable=False,
            )
        except Exception:
            return _error_response(
                status_code=500,
                code="PROCESSOR_INTERNAL",
                category="internal",
                message="processor encountered an internal error",
                retryable=False,
            )

    @router.get("/v1/health")
    async def versioned_health() -> dict[str, str]:
        return _health_payload(processor)

    return router


def _health_payload(processor: LoadForecastProcessor) -> dict[str, str]:
    return {
        "status": "ok" if processor.is_ready() else "unavailable",
        "processor": processor.policy.processor_id,
        "version": processor.policy.processor_version,
        "contract": "aether.data-processing.forecast.v1",
    }


def install_routes(
    app: FastAPI,
    *,
    processor: LoadForecastProcessor,
    max_request_bytes: int = DEFAULT_MAX_REQUEST_BYTES,
    max_processor_concurrency: int | None = None,
    auth_policy: BearerAuthPolicy | None = None,
    include_health_alias: bool = False,
) -> None:
    """Install v1 routes into an existing Edge-Platform FastAPI application."""
    if hasattr(app.state, "aether_load_forecasting_runner"):
        raise ValueError("Aether load forecasting routes are already installed")
    runner = ProcessorRunner(
        max_concurrency=(
            _configured_max_concurrency()
            if max_processor_concurrency is None
            else max_processor_concurrency
        )
    )
    policy = auth_policy or BearerAuthPolicy.from_env()
    app.state.aether_load_forecasting_runner = runner
    app.add_middleware(RequestSizeLimitMiddleware, max_bytes=max_request_bytes)
    app.add_middleware(BearerAuthMiddleware, policy=policy)
    app.include_router(create_router(processor, runner=runner))
    app.router.add_event_handler("shutdown", runner.close)
    if include_health_alias:
        app.add_api_route("/health", lambda: _health_payload(processor), methods=["GET"])


def create_app(
    *,
    processor: LoadForecastProcessor,
    max_request_bytes: int = DEFAULT_MAX_REQUEST_BYTES,
    max_processor_concurrency: int | None = None,
    auth_policy: BearerAuthPolicy | None = None,
) -> FastAPI:
    app = FastAPI(title="Aether Load-Forecasting Processor", version="0.1.0")
    install_routes(
        app,
        processor=processor,
        max_request_bytes=max_request_bytes,
        max_processor_concurrency=max_processor_concurrency,
        auth_policy=auth_policy,
        include_health_alias=True,
    )
    return app
