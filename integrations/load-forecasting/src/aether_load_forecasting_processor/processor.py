"""Load forecast processing orchestration with fail-closed result semantics."""

from __future__ import annotations

import hmac
import math
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from itertools import pairwise
from typing import Any

from .engine import ArtifactProvenance, EngineForecast, ForecastContext, ForecastEngine
from .models import (
    FORECAST_CONTRACT,
    FORECAST_OUTPUT_SCHEMA,
    RESULT_SCHEMA,
    ArtifactProvenanceModel,
    DataProcessingRequest,
    FallbackDescriptor,
    ForecastOutput,
    ForecastPoint,
    ProcessingResult,
    ProcessorDescriptor,
    QuantileValue,
    UnavailableDescriptor,
    compute_input_digest,
    format_utc_timestamp,
    normalize_utc_timestamp,
    parse_utc_timestamp,
)

UTC = timezone.utc


class ProcessorRequestError(ValueError):
    """Stable processor-facing error that the HTTP layer may expose."""

    def __init__(
        self,
        *,
        code: str,
        category: str,
        message: str,
        status_code: int,
        retryable: bool = False,
        request_id: str | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.category = category
        self.public_message = message
        self.status_code = status_code
        self.retryable = retryable
        self.request_id = request_id


@dataclass(frozen=True, slots=True)
class ProcessorPolicy:
    processor_id: str = "load-forecasting-edge"
    processor_version: str = "0.1.0"
    cadence_seconds: int = 900
    history_steps: int = 672
    max_horizon_steps: int = 288
    task_revision: int = 1
    max_input_age_seconds: int = 900
    produced_ttl_seconds: int = 3600
    fallback_ttl_seconds: int = 1800
    retry_after_seconds: int = 900
    allow_persistence_fallback: bool = False

    def __post_init__(self) -> None:
        if any(
            value <= 0
            for value in (
                self.cadence_seconds,
                self.history_steps,
                self.max_horizon_steps,
                self.task_revision,
                self.max_input_age_seconds,
                self.produced_ttl_seconds,
                self.fallback_ttl_seconds,
                self.retry_after_seconds,
            )
        ):
            raise ValueError("processor policy limits must be positive")


_HISTORY_FEATURES = {
    "load": "kW",
    "temp_avg": "Cel",
    "humidity": "%",
    "rain": "mm",
    "quarter_hour": "1",
}
_FUTURE_FEATURES = {
    "temp_avg": "Cel",
    "humidity": "%",
    "rain": "mm",
    "quarter_hour": "1",
}


class LoadForecastProcessor:
    """Validate a complete frame, invoke a model engine, and label every outcome."""

    def __init__(self, engine: ForecastEngine, policy: ProcessorPolicy | None = None) -> None:
        self._engine = engine
        self.policy = policy or ProcessorPolicy()

    def is_ready(self) -> bool:
        """Use an engine readiness seam when present; simple injected engines are ready."""

        readiness = getattr(self._engine, "is_ready", None)
        if readiness is None:
            return True
        try:
            return readiness() is True
        except Exception:
            return False

    def process(self, request: DataProcessingRequest) -> ProcessingResult:
        self._verify_digest(request)
        self._verify_deadline(request)
        self._validate_load_contract(request)
        history_data = self._segment_rows(
            request.frame.history.timestamps, request.frame.history.features
        )
        future = request.frame.future_covariates
        if future is None:  # The contract validator above keeps this branch defensive.
            self._frame_error(request, "future covariates are required")
        forecast_data = self._segment_rows(
            future.timestamps,
            future.features,
            future=True,
        )
        context = ForecastContext(
            request_id=request.request_id,
            binding_id=request.binding.id,
            as_of=request.frame.as_of,
            cadence_seconds=request.frame.cadence_seconds,
            horizon_steps=request.options.horizon_steps,
            artifact_kind=request.artifact.kind if request.artifact else None,
            artifact_family=request.artifact.family if request.artifact else None,
            artifact_version=request.artifact.version if request.artifact else None,
            quantiles=tuple(request.options.quantiles or ()),
        )
        try:
            forecast = self._engine.forecast(
                history_data=history_data,
                forecast_data=forecast_data,
                context=context,
            )
            points = self._validate_engine_forecast(request, forecast)
            artifact = self._validate_artifact(request, forecast.artifact)
        except Exception:
            engine_succeeded = False
        else:
            engine_succeeded = True
        completed_at = self._verify_deadline(request)
        if engine_succeeded:
            return self._produced(request, points, artifact, issued_at=completed_at)
        if self.policy.allow_persistence_fallback:
            return self._persistence_fallback(request, history_data, issued_at=completed_at)
        return self._unavailable(
            request,
            "MODEL_RUNTIME_UNAVAILABLE",
            retryable=True,
            issued_at=completed_at,
        )

    def _verify_digest(self, request: DataProcessingRequest) -> None:
        if not hmac.compare_digest(compute_input_digest(request), request.input_digest):
            raise ProcessorRequestError(
                code="DIGEST_MISMATCH",
                category="invalid_request",
                message="input_digest does not match the canonical request data",
                status_code=400,
                request_id=request.request_id,
            )

    @staticmethod
    def _verify_deadline(request: DataProcessingRequest) -> datetime:
        checked_at = datetime.now(UTC)
        if checked_at >= parse_utc_timestamp(request.deadline):
            raise ProcessorRequestError(
                code="DEADLINE_EXCEEDED",
                category="timeout",
                message="processing deadline has elapsed",
                status_code=504,
                retryable=True,
                request_id=request.request_id,
            )
        return checked_at

    def _validate_load_contract(self, request: DataProcessingRequest) -> None:
        if (
            request.task.id != "energy.site-load-forecast"
            or request.task.revision != self.policy.task_revision
        ):
            self._frame_error(request, "task is not supported by this processor")
        if request.frame.cadence_seconds != self.policy.cadence_seconds:
            self._frame_error(request, "frame cadence is not supported")
        if request.artifact is not None and (
            request.artifact.kind != "model" or request.artifact.family != "site-load"
        ):
            self._frame_error(request, "artifact selector is not supported")
        if request.frame.future_covariates is None:
            self._frame_error(request, "future covariates are required")
        if len(request.frame.history.timestamps) != self.policy.history_steps:
            self._frame_error(request, "history length does not match the commissioned task")
        if request.options.horizon_steps > self.policy.max_horizon_steps:
            self._frame_error(request, "forecast horizon exceeds the commissioned task")
        if request.frame.static_features:
            self._frame_error(request, "static features are not declared for this task revision")

        self._validate_feature_set(
            request, request.frame.history.features, _HISTORY_FEATURES, "history"
        )
        future = request.frame.future_covariates
        self._validate_feature_set(request, future.features, _FUTURE_FEATURES, "future_covariates")
        self._validate_calendar_values(
            request,
            request.frame.history.timestamps,
            request.frame.history.features["quarter_hour"].values,
            "history",
        )
        self._validate_calendar_values(
            request,
            future.timestamps,
            future.features["quarter_hour"].values,
            "future_covariates",
        )
        if len(future.timestamps) != request.options.horizon_steps:
            self._frame_error(request, "future horizon does not match horizon_steps")
        if request.frame.quality.missing_ratio != 0.0:
            self._frame_error(request, "required model inputs must not contain missing samples")
        if request.frame.quality.max_gap_seconds > 2 * request.frame.cadence_seconds:
            self._frame_error(request, "frame max gap exceeds the commissioned task")

        provenance = {(entry.segment, entry.feature): entry for entry in request.frame.provenance}
        as_of = parse_utc_timestamp(request.frame.as_of)
        if any(
            entry.segment == "future_covariates"
            and entry.source_kind not in {"calendar", "constant"}
            and entry.issued_at is None
            for entry in request.frame.provenance
        ):
            self._frame_error(request, "future covariates require issue-time provenance")
        if any(
            as_of - parse_utc_timestamp(entry.watermark)
            > timedelta(seconds=self.policy.max_input_age_seconds)
            for entry in request.frame.provenance
            if entry.source_kind not in {"calendar", "constant"}
        ):
            self._frame_error(request, "a required input source is stale")
        for segment, features in (
            ("history", _HISTORY_FEATURES),
            ("future_covariates", _FUTURE_FEATURES),
        ):
            for feature in features:
                source_kind = provenance[(segment, feature)].source_kind
                if (feature == "quarter_hour") != (source_kind == "calendar"):
                    self._frame_error(
                        request,
                        "calendar and observed feature provenance do not match the task",
                    )

    def _validate_feature_set(self, request, features, expected, segment: str) -> None:
        if set(features) != set(expected):
            self._frame_error(request, f"{segment} feature set does not match the task")
        for name, unit in expected.items():
            feature = features[name]
            if feature.value_type != "number" or feature.unit != unit:
                self._frame_error(request, f"{segment}.{name} has an invalid type or unit")
            if any(
                value is None or quality == "missing"
                for value, quality in zip(feature.values, feature.quality, strict=True)
            ):
                self._frame_error(request, f"{segment}.{name} contains a missing sample")
            values = feature.values
            if name == "humidity" and any(not 0.0 <= value <= 100.0 for value in values):
                self._frame_error(request, f"{segment}.{name} is outside [0, 100]")
            if name == "rain" and any(value < 0.0 for value in values):
                self._frame_error(request, f"{segment}.{name} must be non-negative")
            if name == "quarter_hour" and any(
                not 0.0 <= value <= 95.0 or not float(value).is_integer() for value in values
            ):
                self._frame_error(
                    request,
                    f"{segment}.{name} must be an integer inside [0, 95]",
                )

    def _validate_calendar_values(
        self,
        request: DataProcessingRequest,
        timestamps: list[str],
        values: list[Any],
        segment: str,
    ) -> None:
        for timestamp, value in zip(timestamps, values, strict=True):
            instant = parse_utc_timestamp(timestamp)
            expected = instant.hour * 4 + instant.minute // 15
            if (
                instant.minute % 15 != 0
                or instant.second != 0
                or instant.microsecond != 0
                or value != expected
            ):
                self._frame_error(
                    request,
                    f"{segment}.quarter_hour does not match its UTC timestamp",
                )

    @staticmethod
    def _frame_error(request: DataProcessingRequest, message: str) -> None:
        raise ProcessorRequestError(
            code="FRAME_INVALID",
            category="invalid_data",
            message=message,
            status_code=422,
            request_id=request.request_id,
        )

    @staticmethod
    def _segment_rows(timestamps, features, *, future: bool = False) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        for index, timestamp in enumerate(timestamps):
            row: dict[str, Any] = {"datetime": timestamp}
            for name, feature in features.items():
                row[name] = feature.values[index]
            if future:
                row["load"] = ""
            rows.append(row)
        return rows

    def _validate_engine_forecast(
        self, request: DataProcessingRequest, forecast: EngineForecast
    ) -> list[ForecastPoint]:
        future = request.frame.future_covariates
        if future is None or len(forecast.points) != request.options.horizon_steps:
            raise ValueError("engine returned the wrong point count")
        points: list[ForecastPoint] = []
        expected_quantiles = tuple(request.options.quantiles or ())
        for expected_timestamp, point in zip(future.timestamps, forecast.points, strict=True):
            if point.timestamp != expected_timestamp or not math.isfinite(point.value):
                raise ValueError("engine returned invalid point correlation")
            probabilities = tuple(quantile.probability for quantile in point.quantiles)
            if probabilities != expected_quantiles:
                raise ValueError("engine returned the wrong quantiles")
            values = [quantile.value for quantile in point.quantiles]
            if any(not math.isfinite(value) for value in values):
                raise ValueError("engine returned a non-finite quantile")
            if any(left > right for left, right in pairwise(values)):
                raise ValueError("engine returned crossing quantiles")
            points.append(
                ForecastPoint(
                    timestamp=normalize_utc_timestamp(point.timestamp),
                    value=float(point.value),
                    quantiles=(
                        [
                            QuantileValue(probability=item.probability, value=item.value)
                            for item in point.quantiles
                        ]
                        or None
                    ),
                )
            )
        return points

    @staticmethod
    def _validate_artifact(
        request: DataProcessingRequest, artifact: ArtifactProvenance | None
    ) -> ArtifactProvenanceModel | None:
        if artifact is None:
            return None
        if request.artifact is not None and (
            artifact.kind != request.artifact.kind
            or artifact.family != request.artifact.family
            or (
                request.artifact.version is not None
                and artifact.version != request.artifact.version
            )
            or (
                request.artifact.artifact_digest is not None
                and artifact.artifact_digest != request.artifact.artifact_digest
            )
        ):
            raise ValueError("engine artifact does not match the selector")
        return ArtifactProvenanceModel(
            kind=artifact.kind,
            family=artifact.family,
            version=artifact.version,
            artifact_digest=artifact.artifact_digest,
        )

    def _output(
        self, request: DataProcessingRequest, points: list[ForecastPoint]
    ) -> ForecastOutput:
        return ForecastOutput(
            schema=FORECAST_OUTPUT_SCHEMA,
            kind="forecast",
            target="load",
            unit="kW",
            sign_convention="positive_consumption",
            cadence_seconds=request.frame.cadence_seconds,
            timestamp_semantics="interval_end",
            points=points,
        )

    def _base_result(self, request: DataProcessingRequest) -> dict[str, Any]:
        return {
            "schema": RESULT_SCHEMA,
            "request_id": request.request_id,
            "task": request.task,
            "binding": request.binding,
            "input_digest": request.input_digest,
            "input_watermark": normalize_utc_timestamp(request.frame.quality.input_watermark),
            "processor": ProcessorDescriptor(
                id=self.policy.processor_id,
                version=self.policy.processor_version,
                contract=FORECAST_CONTRACT,
            ),
        }

    def _produced(
        self,
        request: DataProcessingRequest,
        points: list[ForecastPoint],
        artifact: ArtifactProvenanceModel | None,
        *,
        issued_at: datetime,
    ) -> ProcessingResult:
        return ProcessingResult(
            **self._base_result(request),
            status="produced",
            issued_at=format_utc_timestamp(issued_at),
            expires_at=format_utc_timestamp(
                issued_at + timedelta(seconds=self.policy.produced_ttl_seconds)
            ),
            artifact=artifact,
            output=self._output(request, points),
            warnings=[],
        )

    def _persistence_fallback(
        self,
        request: DataProcessingRequest,
        history_data: list[dict[str, Any]],
        *,
        issued_at: datetime,
    ) -> ProcessingResult:
        last_value = history_data[-1]["load"]
        if isinstance(last_value, bool) or not isinstance(last_value, (int, float)):
            return self._unavailable(
                request,
                "INSUFFICIENT_HISTORY",
                retryable=True,
                issued_at=issued_at,
            )
        future = request.frame.future_covariates
        if future is None:
            return self._unavailable(
                request,
                "INSUFFICIENT_HISTORY",
                retryable=True,
                issued_at=issued_at,
            )
        points = [
            ForecastPoint(
                timestamp=normalize_utc_timestamp(timestamp),
                value=float(last_value),
                quantiles=(
                    [
                        QuantileValue(probability=probability, value=float(last_value))
                        for probability in request.options.quantiles or ()
                    ]
                    or None
                ),
            )
            for timestamp in future.timestamps
        ]
        return ProcessingResult(
            **self._base_result(request),
            status="fallback",
            issued_at=format_utc_timestamp(issued_at),
            expires_at=format_utc_timestamp(
                issued_at + timedelta(seconds=self.policy.fallback_ttl_seconds)
            ),
            fallback=FallbackDescriptor(
                strategy="persistence",
                strategy_version="1",
                reason_code="MODEL_UNAVAILABLE",
                source_feature="load",
                based_on_data_through=normalize_utc_timestamp(request.frame.history.timestamps[-1]),
            ),
            output=self._output(request, points),
            warnings=["MODEL_FALLBACK_USED"],
        )

    def _unavailable(
        self,
        request: DataProcessingRequest,
        reason_code: str,
        *,
        retryable: bool,
        issued_at: datetime,
    ) -> ProcessingResult:
        return ProcessingResult(
            **self._base_result(request),
            status="unavailable",
            issued_at=format_utc_timestamp(issued_at),
            unavailable=UnavailableDescriptor(
                reason_code=reason_code,
                retryable=retryable,
                retry_after_seconds=self.policy.retry_after_seconds if retryable else None,
            ),
            warnings=[],
        )
