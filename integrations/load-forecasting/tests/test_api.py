from __future__ import annotations

import asyncio
import json
import re
import threading
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import pytest
from conftest import digest_for
from fastapi import FastAPI
from fastapi.testclient import TestClient
from pydantic import BaseModel

from aether_load_forecasting_processor import api as api_module
from aether_load_forecasting_processor import processor as processor_module
from aether_load_forecasting_processor.api import (
    MEDIA_TYPE,
    BearerAuthPolicy,
    ProcessorBusy,
    ProcessorRunner,
    create_app,
    install_routes,
)
from aether_load_forecasting_processor.engine import (
    ArtifactProvenance,
    CommissionedArtifactBundle,
    EdgePlatformInferenceServiceEngine,
    EngineForecast,
    EngineForecastPoint,
    EngineQuantile,
    compute_artifact_bundle_digest,
)
from aether_load_forecasting_processor.models import DataProcessingRequest
from aether_load_forecasting_processor.processor import (
    LoadForecastProcessor,
    ProcessorPolicy,
    ProcessorRequestError,
)

MILLISECOND_UTC = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$")


class FakeEngine:
    def __init__(self) -> None:
        self.calls: list[dict[str, Any]] = []

    def forecast(self, *, history_data, forecast_data, context) -> EngineForecast:
        self.calls.append(
            {
                "history_data": history_data,
                "forecast_data": forecast_data,
                "context": context,
            }
        )
        last_load = history_data[-1]["load"]
        return EngineForecast(
            points=tuple(
                EngineForecastPoint(timestamp=row["datetime"], value=last_load + index)
                for index, row in enumerate(forecast_data, start=1)
            ),
            artifact=ArtifactProvenance(
                kind="model",
                family="site-load",
                version="v3",
                artifact_digest="sha256:" + "a" * 64,
            ),
        )


class FailingEngine:
    def forecast(self, *, history_data, forecast_data, context) -> EngineForecast:
        raise RuntimeError("model failed at /opt/private/models/site-a.onnx")


class UnreadyEngine(FakeEngine):
    def is_ready(self) -> bool:
        return False


class QuantileEngine:
    def forecast(self, *, history_data, forecast_data, context) -> EngineForecast:
        return EngineForecast(
            points=tuple(
                EngineForecastPoint(
                    timestamp=row["datetime"],
                    value=840.0,
                    quantiles=(
                        EngineQuantile(probability=0.1, value=810.0),
                        EngineQuantile(probability=0.9, value=870.0),
                    ),
                )
                for row in forecast_data
            )
        )


def client_for(engine, *, allow_persistence_fallback: bool = False, max_bytes: int = 1_048_576):
    processor = LoadForecastProcessor(
        engine=engine,
        policy=ProcessorPolicy(
            history_steps=2,
            allow_persistence_fallback=allow_persistence_fallback,
        ),
    )
    return TestClient(
        create_app(processor=processor, max_request_bytes=max_bytes),
        raise_server_exceptions=False,
    )


def add_microseconds_to_request_times(payload: dict[str, Any]) -> None:
    def microseconds(value: str) -> str:
        return f"{value.removesuffix('Z').split('.', maxsplit=1)[0]}.123456Z"

    payload["submitted_at"] = microseconds(payload["submitted_at"])
    payload["deadline"] = microseconds(payload["deadline"])
    frame = payload["frame"]
    frame["as_of"] = microseconds(frame["as_of"])
    frame["quality"]["input_watermark"] = microseconds(frame["quality"]["input_watermark"])
    for segment_name in ("history", "future_covariates"):
        frame[segment_name]["timestamps"] = [
            microseconds(value) for value in frame[segment_name]["timestamps"]
        ]
    for provenance in frame["provenance"]:
        provenance["watermark"] = microseconds(provenance["watermark"])
        if "issued_at" in provenance:
            provenance["issued_at"] = microseconds(provenance["issued_at"])
    payload["input_digest"] = digest_for(payload)


def test_processes_complete_frame_end_to_end(valid_payload: dict[str, Any]) -> None:
    engine = FakeEngine()

    response = client_for(engine).post(
        "/v1/process",
        content=json.dumps(valid_payload),
        headers={"content-type": MEDIA_TYPE, "accept": MEDIA_TYPE},
    )

    assert response.status_code == 200, response.text
    assert response.headers["content-type"] == MEDIA_TYPE
    body = response.json()
    assert body["schema"] == "aether.data-processing.result.v1"
    assert body["status"] == "produced"
    assert body["request_id"] == valid_payload["request_id"]
    assert body["task"] == valid_payload["task"]
    assert body["binding"] == valid_payload["binding"]
    assert body["input_digest"] == valid_payload["input_digest"]
    assert body["artifact"]["artifact_digest"] == "sha256:" + "a" * 64
    assert [point["value"] for point in body["output"]["points"]] == [841.0, 842.0]
    assert len(engine.calls) == 1
    call = engine.calls[0]
    assert call["history_data"][0] == {
        "datetime": "2099-07-11T11:45:00Z",
        "load": 835.0,
        "temp_avg": 31.2,
        "humidity": 63.0,
        "rain": 0.0,
        "quarter_hour": 47,
    }
    assert call["forecast_data"][0] == {
        "datetime": "2099-07-11T12:15:00Z",
        "temp_avg": 32.1,
        "humidity": 61.0,
        "rain": 0.0,
        "quarter_hour": 49,
        "load": "",
    }
    assert not any(
        forbidden in repr(call).lower()
        for forbidden in ("callback", "endpoint", "credential", "influx", "shm")
    )


def test_local_artifact_digest_flows_through_full_http_boundary(
    valid_payload: dict[str, Any], tmp_path: Path
) -> None:
    model = tmp_path / "model.onnx"
    scaler = tmp_path / "scaler.json"
    model.write_bytes(b"real commissioned model")
    scaler.write_bytes(b'{"scale": 2.0}')
    files = {"model": model, "scaler": scaler}
    bundle = CommissionedArtifactBundle(
        files=files,
        expected_digest=compute_artifact_bundle_digest(files),
    )

    class LegacyService:
        @staticmethod
        def run_inference(_pre_out, _raw_event):
            return {
                "statusCode": 200,
                "body": {
                    "model_version": "v3",
                    "predictions": [
                        {"ts": "2099-07-11T12:15:00Z", "value": 841.0},
                        {"ts": "2099-07-11T12:30:00Z", "value": 842.0},
                    ],
                },
            }

    engine = EdgePlatformInferenceServiceEngine(
        LegacyService(),
        artifact_bundles={("model", "site-load", "v3"): bundle},
        artifact_file_resolver=lambda _context: bundle.files,
        readiness_probe=lambda: True,
    )
    client = client_for(engine)
    valid_payload["artifact"]["artifact_digest"] = bundle.actual_digest
    valid_payload["input_digest"] = digest_for(valid_payload)

    produced = client.post("/v1/process", json=valid_payload)

    assert produced.status_code == 200, produced.text
    assert produced.json()["status"] == "produced"
    assert produced.json()["artifact"]["artifact_digest"] == bundle.actual_digest

    valid_payload["artifact"]["artifact_digest"] = "sha256:" + "f" * 64
    valid_payload["input_digest"] = digest_for(valid_payload)
    mismatched = client.post("/v1/process", json=valid_payload)

    assert mismatched.status_code == 200
    assert mismatched.json()["status"] == "unavailable"
    assert "artifact" not in mismatched.json()
    assert "output" not in mismatched.json()


def test_rejects_explicit_null_for_optional_wire_field(
    valid_payload: dict[str, Any],
) -> None:
    valid_payload["artifact"] = None

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_digest_normalizes_equivalent_timestamp_lexemes(
    valid_payload: dict[str, Any],
) -> None:
    original_digest = valid_payload["input_digest"]
    valid_payload["frame"]["as_of"] = "2099-07-11T12:00:00.000Z"

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 200, response.text
    assert valid_payload["input_digest"] == original_digest


def test_health_is_local_and_does_not_invoke_engine() -> None:
    engine = FakeEngine()

    response = client_for(engine).get("/health")

    assert response.status_code == 200
    assert response.headers["content-type"] == "application/json"
    assert response.json() == {
        "status": "ok",
        "processor": "load-forecasting-edge",
        "version": "0.1.0",
        "contract": "aether.data-processing.forecast.v1",
    }
    assert engine.calls == []


def test_versioned_health_is_available_when_mounted() -> None:
    response = client_for(FakeEngine()).get("/v1/health")

    assert response.status_code == 200
    assert response.json()["contract"] == "aether.data-processing.forecast.v1"


def test_health_reports_unavailable_when_engine_is_not_commissioned() -> None:
    response = client_for(UnreadyEngine()).get("/v1/health")

    assert response.status_code == 200
    assert response.json()["status"] == "unavailable"


def test_process_route_requires_configured_bearer_but_health_stays_local(
    valid_payload: dict[str, Any],
) -> None:
    app = create_app(
        processor=LoadForecastProcessor(
            engine=FakeEngine(),
            policy=ProcessorPolicy(history_steps=2),
        ),
        auth_policy=BearerAuthPolicy(token="commissioned-secret-0123456789abcdef", required=True),
    )
    client = TestClient(app)

    missing = client.post("/v1/process", json=valid_payload)
    wrong = client.post(
        "/v1/process",
        json=valid_payload,
        headers={"authorization": "Bearer wrong-secret"},
    )
    accepted = client.post(
        "/v1/process",
        json=valid_payload,
        headers={"authorization": "Bearer commissioned-secret-0123456789abcdef"},
    )

    assert missing.status_code == 401
    assert wrong.status_code == 401
    assert missing.json()["code"] == "AUTHENTICATION_REQUIRED"
    assert missing.json()["category"] == "authorization"
    assert missing.headers["www-authenticate"] == "Bearer"
    assert accepted.status_code == 200, accepted.text
    assert client.get("/health").status_code == 200
    assert client.get("/v1/health").status_code == 200


def test_required_bearer_token_fails_closed_when_secret_is_missing() -> None:
    with pytest.raises(ValueError, match="bearer token"):
        BearerAuthPolicy(token=None, required=True)
    with pytest.raises(ValueError, match="bearer token"):
        BearerAuthPolicy(token="too-short", required=True)


@pytest.mark.parametrize("token", ["contains:colon", "unicode-密码", "has space"])
def test_bearer_policy_rejects_tokens_the_rust_client_cannot_send(token: str) -> None:
    with pytest.raises(ValueError, match="bearer token"):
        BearerAuthPolicy(token=token)


def test_bearer_policy_loads_strict_environment(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AETHER_LOAD_FORECASTING_BEARER_TOKEN", "secret-from-env-0123456789abcdef")
    monkeypatch.setenv("AETHER_LOAD_FORECASTING_REQUIRE_AUTH", "true")

    policy = BearerAuthPolicy.from_env()

    assert policy.token == "secret-from-env-0123456789abcdef"
    assert policy.required is True

    monkeypatch.setenv("AETHER_LOAD_FORECASTING_REQUIRE_AUTH", "sometimes")
    with pytest.raises(ValueError, match="REQUIRE_AUTH"):
        BearerAuthPolicy.from_env()


def test_bearer_credentials_use_constant_time_comparison(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    comparisons: list[tuple[str, str]] = []

    def record_compare(candidate: str, expected: str) -> bool:
        comparisons.append((candidate, expected))
        return candidate == expected

    monkeypatch.setattr(api_module.hmac, "compare_digest", record_compare)
    policy = BearerAuthPolicy(token="commissioned-secret-0123456789abcdef")

    assert policy.authorizes(b"Bearer commissioned-secret-0123456789abcdef") is True
    assert comparisons == [
        (
            "commissioned-secret-0123456789abcdef",
            "commissioned-secret-0123456789abcdef",
        )
    ]


def test_empty_optional_bearer_environment_is_normalized_but_required_still_fails(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("AETHER_LOAD_FORECASTING_BEARER_TOKEN", "")
    monkeypatch.setenv("AETHER_LOAD_FORECASTING_REQUIRE_AUTH", "false")
    assert BearerAuthPolicy.from_env().token is None

    monkeypatch.setenv("AETHER_LOAD_FORECASTING_REQUIRE_AUTH", "true")
    with pytest.raises(ValueError, match="bearer token"):
        BearerAuthPolicy.from_env()


def test_processor_runner_keeps_slot_until_cancelled_background_work_finishes() -> None:
    started = threading.Event()
    release = threading.Event()

    def blocking_process() -> str:
        started.set()
        if not release.wait(timeout=5):
            raise RuntimeError("test timed out")
        return "complete"

    async def scenario() -> None:
        runner = ProcessorRunner(max_concurrency=1)
        first = asyncio.create_task(runner.run(blocking_process))
        assert await asyncio.to_thread(started.wait, 1)
        first.cancel()
        with pytest.raises(asyncio.CancelledError):
            await first

        with pytest.raises(ProcessorBusy):
            await runner.run(lambda: "must not run")

        release.set()
        for _ in range(100):
            try:
                assert await runner.run(lambda: "next") == "next"
                break
            except ProcessorBusy:
                await asyncio.sleep(0.01)
        else:
            pytest.fail("processor slot was not released after background completion")
        runner.close()

    asyncio.run(scenario())


def test_process_route_returns_typed_429_while_model_slot_is_occupied(
    valid_payload: dict[str, Any],
) -> None:
    started = threading.Event()
    release = threading.Event()

    class BlockingEngine(FakeEngine):
        def forecast(self, *, history_data, forecast_data, context) -> EngineForecast:
            started.set()
            if not release.wait(timeout=5):
                raise RuntimeError("test timed out")
            return super().forecast(
                history_data=history_data,
                forecast_data=forecast_data,
                context=context,
            )

    app = create_app(
        processor=LoadForecastProcessor(
            engine=BlockingEngine(),
            policy=ProcessorPolicy(history_steps=2),
        ),
        max_processor_concurrency=1,
    )
    client = TestClient(app, raise_server_exceptions=False)
    with ThreadPoolExecutor(max_workers=1) as requests:
        first = requests.submit(client.post, "/v1/process", json=deepcopy(valid_payload))
        assert started.wait(timeout=1)
        busy = client.post("/v1/process", json=deepcopy(valid_payload))
        release.set()
        completed = first.result(timeout=2)

    assert busy.status_code == 429
    assert busy.json() == {
        "schema": "aether.data-processing.error.v1",
        "request_id": valid_payload["request_id"],
        "code": "PROCESSOR_BUSY",
        "category": "capacity",
        "message": "processor concurrency limit is occupied",
        "retryable": True,
    }
    assert busy.headers["retry-after"] == "1"
    assert completed.status_code == 200


def test_routes_install_into_existing_edge_platform_app(valid_payload: dict[str, Any]) -> None:
    app = FastAPI()
    install_routes(
        app,
        processor=LoadForecastProcessor(
            engine=FakeEngine(),
            policy=ProcessorPolicy(history_steps=2),
        ),
    )

    response = TestClient(app).post("/v1/process", json=valid_payload)

    assert response.status_code == 200, response.text
    assert response.json()["status"] == "produced"


def test_route_installation_does_not_replace_legacy_app_error_contract() -> None:
    class LegacyPayload(BaseModel):
        value: int

    app = FastAPI()

    @app.post("/legacy")
    async def legacy(_payload: LegacyPayload) -> dict[str, bool]:
        return {"ok": True}

    install_routes(
        app,
        processor=LoadForecastProcessor(
            engine=FakeEngine(),
            policy=ProcessorPolicy(history_steps=2),
        ),
    )

    response = TestClient(app).post("/legacy", json={})

    assert response.status_code == 422
    assert "detail" in response.json()
    assert "code" not in response.json()


def test_quantiles_are_preserved_when_engine_supports_them(
    valid_payload: dict[str, Any],
) -> None:
    valid_payload["options"]["quantiles"] = [0.1, 0.9]
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(QuantileEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 200
    assert response.json()["output"]["points"][0]["quantiles"] == [
        {"probability": 0.1, "value": 810.0},
        {"probability": 0.9, "value": 870.0},
    ]


def test_rejects_missing_required_model_sample(valid_payload: dict[str, Any]) -> None:
    valid_payload["frame"]["history"]["features"]["load"]["values"][0] = None
    valid_payload["frame"]["history"]["features"]["load"]["quality"][0] = "missing"
    valid_payload["frame"]["quality"]["missing_ratio"] = 0.025
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_non_finite_number(valid_payload: dict[str, Any]) -> None:
    raw = json.dumps(valid_payload).replace("835.0", "NaN", 1)

    response = client_for(FakeEngine()).post(
        "/v1/process",
        content=raw,
        headers={"content-type": "application/json"},
    )

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_unsupported_process_media_type(valid_payload: dict[str, Any]) -> None:
    response = client_for(FakeEngine()).post(
        "/v1/process",
        content=json.dumps(valid_payload),
        headers={"content-type": "text/plain"},
    )

    assert response.status_code == 415
    assert response.headers["content-type"] == MEDIA_TYPE
    assert response.json()["code"] == "MEDIA_TYPE_UNSUPPORTED"


def test_rejects_out_of_order_timestamps(valid_payload: dict[str, Any]) -> None:
    timestamps = valid_payload["frame"]["history"]["timestamps"]
    timestamps[0], timestamps[1] = timestamps[1], timestamps[0]
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_history_length_that_does_not_match_commissioned_policy(
    valid_payload: dict[str, Any],
) -> None:
    valid_payload["frame"]["history"]["timestamps"].pop(0)
    for feature in valid_payload["frame"]["history"]["features"].values():
        feature["values"].pop(0)
        feature["quality"].pop(0)
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_future_covariate_provenance_issued_after_frame_cutoff(
    valid_payload: dict[str, Any],
) -> None:
    future_provenance = next(
        item
        for item in valid_payload["frame"]["provenance"]
        if item["segment"] == "future_covariates" and item["feature"] == "temp_avg"
    )
    future_provenance["issued_at"] = "2099-07-11T12:00:01Z"
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_future_covariate_without_issue_time(
    valid_payload: dict[str, Any],
) -> None:
    future_provenance = next(
        item
        for item in valid_payload["frame"]["provenance"]
        if item["segment"] == "future_covariates" and item["feature"] == "humidity"
    )
    del future_provenance["issued_at"]
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_stale_required_source_even_when_aggregate_watermark_is_fresh(
    valid_payload: dict[str, Any],
) -> None:
    humidity = next(
        entry
        for entry in valid_payload["frame"]["provenance"]
        if entry["segment"] == "history" and entry["feature"] == "humidity"
    )
    humidity["watermark"] = "2099-07-11T11:44:59Z"
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


@pytest.mark.parametrize(
    "mutation",
    [
        "missing_calendar",
        "duplicate_key",
        "unknown_key",
        "watermark_after_cutoff",
        "aggregate_watermark_mismatch",
        "wrong_calendar_kind",
        "ordinary_as_calendar",
    ],
)
def test_rejects_non_exact_or_inconsistent_provenance(
    valid_payload: dict[str, Any], mutation: str
) -> None:
    provenance = valid_payload["frame"]["provenance"]
    if mutation == "missing_calendar":
        provenance[:] = [
            entry
            for entry in provenance
            if not (entry["segment"] == "history" and entry["feature"] == "quarter_hour")
        ]
    elif mutation == "duplicate_key":
        duplicate = deepcopy(provenance[0])
        duplicate["source_ref"] = "another.semantic.source"
        provenance.append(duplicate)
    elif mutation == "unknown_key":
        unknown = deepcopy(provenance[0])
        unknown["feature"] = "undeclared"
        provenance.append(unknown)
    elif mutation == "watermark_after_cutoff":
        provenance[0]["watermark"] = "2099-07-11T12:00:01Z"
    elif mutation == "wrong_calendar_kind":
        next(
            entry
            for entry in provenance
            if entry["segment"] == "history" and entry["feature"] == "quarter_hour"
        ).update(source_kind="history", watermark="2099-07-11T11:59:58Z")
    elif mutation == "ordinary_as_calendar":
        provenance[0]["source_kind"] = "calendar"
        valid_payload["frame"]["quality"]["input_watermark"] = "2099-07-11T11:50:00Z"
    else:
        valid_payload["frame"]["quality"]["input_watermark"] = "2099-07-11T11:50:00Z"
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


@pytest.mark.parametrize(
    ("path", "value"),
    [
        (("frame", "history", "features", "load", "unit"), "W"),
        (("frame", "history", "features", "load", "value_type"), "string"),
        (("frame", "future_covariates", "features", "rain", "unit"), "cm"),
    ],
)
def test_rejects_wrong_feature_type_or_unit(
    valid_payload: dict[str, Any], path: tuple[str, ...], value: str
) -> None:
    current: Any = valid_payload
    for component in path[:-1]:
        current = current[component]
    current[path[-1]] = value
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


@pytest.mark.parametrize(
    ("segment", "feature", "value"),
    [
        ("history", "humidity", 100.1),
        ("future_covariates", "humidity", -0.1),
        ("history", "rain", -0.1),
        ("future_covariates", "quarter_hour", 1.5),
        ("history", "quarter_hour", 96.0),
    ],
)
def test_rejects_task_range_violations(
    valid_payload: dict[str, Any], segment: str, feature: str, value: float
) -> None:
    valid_payload["frame"][segment]["features"][feature]["values"][0] = value
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_calendar_value_that_does_not_match_its_timestamp(
    valid_payload: dict[str, Any],
) -> None:
    valid_payload["frame"]["history"]["features"]["quarter_hour"]["values"][0] = 45.0
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_unknown_feature(valid_payload: dict[str, Any]) -> None:
    valid_payload["frame"]["history"]["features"]["voltage"] = deepcopy(
        valid_payload["frame"]["history"]["features"]["load"]
    )
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_reverse_read_fields_even_if_digest_is_recomputed(
    valid_payload: dict[str, Any],
) -> None:
    valid_payload["source_endpoint"] = "http://aether.internal/history"
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    body = response.json()
    assert body["code"] == "FRAME_INVALID"
    assert "aether.internal" not in response.text


def test_rejects_digest_mismatch(valid_payload: dict[str, Any]) -> None:
    valid_payload["input_digest"] = "sha256:" + "0" * 64

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 400
    assert response.headers["content-type"] == MEDIA_TYPE
    assert response.json()["code"] == "DIGEST_MISMATCH"


def test_produced_response_normalizes_every_timestamp_to_utc_milliseconds(
    valid_payload: dict[str, Any],
) -> None:
    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 200
    body = response.json()
    timestamps = [
        body["issued_at"],
        body["expires_at"],
        body["input_watermark"],
        *(point["timestamp"] for point in body["output"]["points"]),
    ]
    assert all(MILLISECOND_UTC.fullmatch(value) for value in timestamps)
    assert body["input_watermark"] == "2099-07-11T12:00:00.000Z"
    assert body["output"]["points"][0]["timestamp"] == "2099-07-11T12:15:00.000Z"


def test_fallback_and_unavailable_timestamps_are_utc_milliseconds(
    valid_payload: dict[str, Any],
) -> None:
    fallback = client_for(FailingEngine(), allow_persistence_fallback=True).post(
        "/v1/process", json=valid_payload
    )
    unavailable = client_for(FailingEngine()).post("/v1/process", json=valid_payload)

    assert fallback.headers["content-type"] == MEDIA_TYPE
    assert unavailable.headers["content-type"] == MEDIA_TYPE
    fallback_body = fallback.json()
    unavailable_body = unavailable.json()
    fallback_timestamps = [
        fallback_body["issued_at"],
        fallback_body["expires_at"],
        fallback_body["input_watermark"],
        fallback_body["fallback"]["based_on_data_through"],
        *(point["timestamp"] for point in fallback_body["output"]["points"]),
    ]
    unavailable_timestamps = [
        unavailable_body["issued_at"],
        unavailable_body["input_watermark"],
    ]
    assert all(MILLISECOND_UTC.fullmatch(value) for value in fallback_timestamps)
    assert all(MILLISECOND_UTC.fullmatch(value) for value in unavailable_timestamps)


def test_rejects_timestamp_precision_beyond_v1_milliseconds(
    valid_payload: dict[str, Any],
) -> None:
    add_microseconds_to_request_times(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.headers["content-type"] == MEDIA_TYPE
    assert response.json()["code"] == "FRAME_INVALID"


def test_rejects_elapsed_deadline(valid_payload: dict[str, Any]) -> None:
    valid_payload["submitted_at"] = "2020-07-11T12:00:01Z"
    valid_payload["deadline"] = "2020-07-11T12:00:06Z"

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 504
    assert response.json()["code"] == "DEADLINE_EXCEEDED"


@pytest.mark.parametrize(
    ("engine", "allow_fallback"),
    [(FakeEngine(), False), (FailingEngine(), True)],
)
def test_never_publishes_result_after_engine_crosses_deadline(
    valid_payload: dict[str, Any],
    monkeypatch: pytest.MonkeyPatch,
    engine,
    allow_fallback: bool,
) -> None:
    before_deadline = datetime(2099, 7, 11, 12, 0, 5, tzinfo=timezone.utc)
    after_deadline = datetime(2099, 7, 11, 12, 0, 7, tzinfo=timezone.utc)

    class SequencedDatetime(datetime):
        values = iter((before_deadline, after_deadline))

        @classmethod
        def now(cls, tz=None):
            value = next(cls.values)
            return value if tz is None else value.astimezone(tz)

    monkeypatch.setattr(processor_module, "datetime", SequencedDatetime)
    processor = LoadForecastProcessor(
        engine=engine,
        policy=ProcessorPolicy(
            history_steps=2,
            allow_persistence_fallback=allow_fallback,
        ),
    )
    request = DataProcessingRequest.model_validate(valid_payload)

    with pytest.raises(ProcessorRequestError) as error:
        processor.process(request)

    assert error.value.code == "DEADLINE_EXCEEDED"
    assert error.value.status_code == 504


@pytest.mark.parametrize(
    "mutation",
    [
        lambda payload: payload["task"].update(id="energy.site-pv-forecast"),
        lambda payload: payload["task"].update(revision=2),
        lambda payload: payload["artifact"].update(family="other-model"),
        lambda payload: payload["frame"]["static_features"].update(
            rated_power={
                "value_type": "number",
                "unit": "kW",
                "value": 2500.0,
                "quality": "good",
            }
        ),
        lambda payload: payload["frame"]["future_covariates"]["timestamps"].pop(),
        lambda payload: payload["frame"]["provenance"].pop(0),
    ],
)
def test_rejects_uncommissioned_load_contract_variants(
    valid_payload: dict[str, Any], mutation
) -> None:
    mutation(valid_payload)
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_declared_but_uncommissioned_static_feature_is_a_typed_rejection(
    valid_payload: dict[str, Any],
) -> None:
    valid_payload["frame"]["static_features"]["rated_power"] = {
        "value_type": "number",
        "unit": "kW",
        "value": 2500.0,
        "quality": "good",
    }
    valid_payload["frame"]["provenance"].append(
        {
            "segment": "static_features",
            "feature": "rated_power",
            "source_kind": "constant",
            "source_ref": "energy.site.rated_power",
            "watermark": valid_payload["frame"]["as_of"],
        }
    )
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_provenance_source_kind_cannot_bypass_segment_freshness_policy(
    valid_payload: dict[str, Any],
) -> None:
    valid_payload["frame"]["provenance"][0]["source_kind"] = "constant"

    response = client_for(FakeEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 422
    assert response.json()["code"] == "FRAME_INVALID"


def test_engine_failure_is_unavailable_without_internal_details(
    valid_payload: dict[str, Any],
) -> None:
    response = client_for(FailingEngine()).post("/v1/process", json=valid_payload)

    assert response.status_code == 200
    body = response.json()
    assert body["status"] == "unavailable"
    assert body["request_id"] == valid_payload["request_id"]
    assert body["input_digest"] == valid_payload["input_digest"]
    assert body["unavailable"] == {
        "reason_code": "MODEL_RUNTIME_UNAVAILABLE",
        "retryable": True,
        "retry_after_seconds": 900,
    }
    assert "private" not in response.text
    assert "onnx" not in response.text
    assert "trace" not in response.text.lower()
    assert "output" not in body


def test_approved_persistence_fallback_uses_request_history(
    valid_payload: dict[str, Any],
) -> None:
    response = client_for(FailingEngine(), allow_persistence_fallback=True).post(
        "/v1/process", json=valid_payload
    )

    assert response.status_code == 200
    body = response.json()
    assert body["status"] == "fallback"
    assert body["fallback"]["strategy"] == "persistence"
    assert body["fallback"]["reason_code"] == "MODEL_UNAVAILABLE"
    assert body["fallback"]["source_feature"] == "load"
    assert body["fallback"]["based_on_data_through"] == "2099-07-11T12:00:00.000Z"
    assert [point["value"] for point in body["output"]["points"]] == [840.0, 840.0]
    assert body["warnings"] == ["MODEL_FALLBACK_USED"]


def test_persistence_fallback_echoes_requested_quantiles_at_the_source_value(
    valid_payload: dict[str, Any],
) -> None:
    valid_payload["options"]["quantiles"] = [0.1, 0.9]
    valid_payload["input_digest"] = digest_for(valid_payload)

    response = client_for(FailingEngine(), allow_persistence_fallback=True).post(
        "/v1/process", json=valid_payload
    )

    assert response.status_code == 200
    assert [point["quantiles"] for point in response.json()["output"]["points"]] == [
        [
            {"probability": 0.1, "value": 840.0},
            {"probability": 0.9, "value": 840.0},
        ],
        [
            {"probability": 0.1, "value": 840.0},
            {"probability": 0.9, "value": 840.0},
        ],
    ]


def test_rejects_payload_over_configured_limit(valid_payload: dict[str, Any]) -> None:
    response = client_for(FakeEngine(), max_bytes=512).post("/v1/process", json=valid_payload)

    assert response.status_code == 413
    assert response.json() == {
        "schema": "aether.data-processing.error.v1",
        "code": "FRAME_TOO_LARGE",
        "category": "resource_limit",
        "message": "request body exceeds configured limit",
        "retryable": False,
    }


def test_rejects_invalid_content_length_before_parsing() -> None:
    response = client_for(FakeEngine()).post(
        "/v1/process",
        content=b"{}",
        headers={"content-type": "application/json", "content-length": "not-a-number"},
    )

    assert response.status_code == 400
    assert response.json()["code"] == "CONTENT_LENGTH_INVALID"


def test_unexpected_transport_error_is_redacted(valid_payload: dict[str, Any]) -> None:
    processor = LoadForecastProcessor(engine=FakeEngine())

    def explode(_request):
        raise RuntimeError("secret path /private/model.onnx")

    processor.process = explode  # type: ignore[method-assign]
    client = TestClient(create_app(processor=processor), raise_server_exceptions=False)

    response = client.post("/v1/process", json=valid_payload)

    assert response.status_code == 500
    assert response.json()["code"] == "PROCESSOR_INTERNAL"
    assert "private" not in response.text
