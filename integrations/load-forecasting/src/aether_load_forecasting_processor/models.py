"""Strict Aether Data Processing v1 wire models."""

from __future__ import annotations

import hashlib
import math
import re
from datetime import datetime, timezone
from itertools import pairwise
from typing import Annotated, Any, Literal
from uuid import UUID

import rfc8785
from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    StrictBool,
    StrictFloat,
    StrictInt,
    StrictStr,
    field_serializer,
    field_validator,
    model_validator,
)

REQUEST_SCHEMA = "aether.data-processing.request.v1"
FRAME_SCHEMA = "aether.processing-frame.v1"
RESULT_SCHEMA = "aether.data-processing.result.v1"
ERROR_SCHEMA = "aether.data-processing.error.v1"
FORECAST_OUTPUT_SCHEMA = "aether.data-processing.output.forecast.v1"
FORECAST_CONTRACT = "aether.data-processing.forecast.v1"

_UTC_TIMESTAMP = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$")
_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
_IDENTIFIER = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$")
_FEATURE_NAME = re.compile(r"^[^\x00-\x1f\x7f]{1,128}$")
_SOURCE_REF = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,2047}$")
_STABLE_CODE = re.compile(r"^[A-Z][A-Z0-9_]{0,127}$")
UTC = timezone.utc

Identifier = Annotated[StrictStr, Field(min_length=1, max_length=256)]
FeatureName = Annotated[StrictStr, Field(min_length=1, max_length=128)]
Unit = Annotated[StrictStr, Field(min_length=1, max_length=64)]
SourceRef = Annotated[StrictStr, Field(min_length=1, max_length=2048)]
StableCode = Annotated[StrictStr, Field(min_length=1, max_length=128)]
ShortText = Annotated[StrictStr, Field(min_length=1, max_length=256)]
Timestamp = Annotated[StrictStr, Field(min_length=20, max_length=40)]
Digest = Annotated[StrictStr, Field(min_length=71, max_length=71)]
Scalar = StrictBool | StrictInt | StrictFloat | StrictStr | None


def parse_utc_timestamp(value: str) -> datetime:
    """Parse the contract's RFC 3339 UTC subset without accepting local time."""
    if not _UTC_TIMESTAMP.fullmatch(value):
        raise ValueError(
            "timestamp must be an RFC 3339 UTC value ending in Z with at most millisecond precision"
        )
    try:
        parsed = datetime.fromisoformat(f"{value[:-1]}+00:00")
    except ValueError as exc:
        raise ValueError("timestamp is not a valid calendar time") from exc
    if parsed.utcoffset() != UTC.utcoffset(parsed):
        raise ValueError("timestamp must be UTC")
    return parsed


def format_utc_timestamp(value: datetime) -> str:
    normalized = value.astimezone(UTC)
    return normalized.isoformat(timespec="milliseconds").replace("+00:00", "Z")


def normalize_utc_timestamp(value: str) -> str:
    """Render an accepted wire timestamp at the Rust codec's millisecond precision."""
    return format_utc_timestamp(parse_utc_timestamp(value))


def canonical_digest_timestamp(value: str) -> str:
    """Match Rust `SecondsFormat::AutoSi` for the canonical digest basis."""
    parsed = parse_utc_timestamp(value)
    timespec = "seconds" if parsed.microsecond == 0 else "milliseconds"
    return parsed.isoformat(timespec=timespec).replace("+00:00", "Z")


class ContractModel(BaseModel):
    """Reject undeclared fields and serialize internal aliases as wire names."""

    model_config = ConfigDict(
        extra="forbid",
        strict=True,
        allow_inf_nan=False,
        populate_by_name=True,
        serialize_by_alias=True,
    )


class SeriesFeature(ContractModel):
    value_type: Literal["number", "string", "boolean"]
    unit: Unit | None = None
    values: Annotated[list[Scalar], Field(min_length=1, max_length=20_000)]
    quality: Annotated[
        list[Literal["good", "uncertain", "substituted", "missing"]],
        Field(min_length=1, max_length=20_000),
    ]

    @model_validator(mode="after")
    def validate_cells(self) -> SeriesFeature:
        if len(self.values) != len(self.quality):
            raise ValueError("feature values and quality arrays must have equal length")
        if self.value_type == "number" and self.unit is None:
            raise ValueError("numeric features require an explicit unit")
        if self.value_type != "number" and self.unit is not None:
            raise ValueError("non-numeric features must not declare a unit")
        for value, quality in zip(self.values, self.quality, strict=True):
            if quality == "missing":
                if value is not None:
                    raise ValueError("missing quality requires a null value")
                continue
            if value is None:
                raise ValueError("null values require missing quality")
            if self.value_type == "number":
                if isinstance(value, bool) or not isinstance(value, (int, float)):
                    raise ValueError("number feature contains a non-number value")
                if not math.isfinite(value):
                    raise ValueError("number feature values must be finite")
            elif self.value_type == "string" and not isinstance(value, str):
                raise ValueError("string feature contains a non-string value")
            elif self.value_type == "boolean" and not isinstance(value, bool):
                raise ValueError("boolean feature contains a non-boolean value")
        return self


class StaticFeature(ContractModel):
    value_type: Literal["number", "string", "boolean"]
    unit: Unit | None = None
    value: Scalar
    quality: Literal["good", "uncertain", "substituted", "missing"]

    @model_validator(mode="after")
    def validate_cell(self) -> StaticFeature:
        SeriesFeature(
            value_type=self.value_type,
            unit=self.unit,
            values=[self.value],
            quality=[self.quality],
        )
        return self


class FrameSegment(ContractModel):
    timestamps: Annotated[list[Timestamp], Field(min_length=1, max_length=20_000)]
    features: Annotated[dict[str, SeriesFeature], Field(min_length=1, max_length=128)]

    @field_validator("timestamps")
    @classmethod
    def timestamps_are_increasing(cls, values: list[str]) -> list[str]:
        parsed = [parse_utc_timestamp(value) for value in values]
        if any(left >= right for left, right in pairwise(parsed)):
            raise ValueError("timestamps must be strictly increasing")
        return values

    @field_validator("features")
    @classmethod
    def feature_names_are_valid(cls, values: dict[str, SeriesFeature]) -> dict[str, SeriesFeature]:
        if any(not _FEATURE_NAME.fullmatch(name) for name in values):
            raise ValueError("feature name is invalid")
        return values

    @model_validator(mode="after")
    def arrays_match_time_axis(self) -> FrameSegment:
        if any(len(feature.values) != len(self.timestamps) for feature in self.features.values()):
            raise ValueError("every feature array must match the timestamp count")
        return self


class FrameQuality(ContractModel):
    input_watermark: Timestamp
    missing_ratio: Annotated[float, Field(ge=0.0, le=1.0, allow_inf_nan=False)]
    max_gap_seconds: Annotated[StrictInt, Field(ge=0)]
    live_tail_included: StrictBool
    substituted_samples: Annotated[StrictInt, Field(ge=0)]

    @field_validator("input_watermark")
    @classmethod
    def watermark_is_utc(cls, value: str) -> str:
        parse_utc_timestamp(value)
        return value


class ProvenanceEntry(ContractModel):
    segment: Literal["history", "future_covariates", "static_features"]
    feature: FeatureName
    source_kind: Literal["history", "live", "history_and_live", "covariate", "calendar", "constant"]
    source_ref: SourceRef | None = None
    watermark: Timestamp
    issued_at: Timestamp | None = None

    @field_validator("feature")
    @classmethod
    def feature_name_is_valid(cls, value: str) -> str:
        if not _FEATURE_NAME.fullmatch(value):
            raise ValueError("provenance feature name is invalid")
        return value

    @field_validator("source_ref")
    @classmethod
    def source_ref_is_semantic(cls, value: str | None) -> str | None:
        if value is not None and not _SOURCE_REF.fullmatch(value):
            raise ValueError("source_ref must be a semantic logical name")
        return value

    @field_validator("watermark", "issued_at")
    @classmethod
    def provenance_time_is_utc(cls, value: str | None) -> str | None:
        if value is not None:
            parse_utc_timestamp(value)
        return value


class ProcessingFrame(ContractModel):
    schema_: Literal[FRAME_SCHEMA] = Field(alias="schema")
    as_of: Timestamp
    cadence_seconds: Annotated[StrictInt, Field(gt=0, le=86_400)]
    history: FrameSegment
    future_covariates: FrameSegment | None = None
    static_features: Annotated[dict[str, StaticFeature], Field(max_length=128)] = Field(
        default_factory=dict
    )
    quality: FrameQuality
    provenance: Annotated[list[ProvenanceEntry], Field(min_length=1, max_length=512)]

    @model_validator(mode="after")
    def validate_time_boundaries(self) -> ProcessingFrame:
        as_of = parse_utc_timestamp(self.as_of)
        history = [parse_utc_timestamp(value) for value in self.history.timestamps]
        if any(value > as_of for value in history):
            raise ValueError("historical timestamps must not be after as_of")
        self._validate_cadence(history)
        if not history or history[-1] != as_of:
            raise ValueError("interval-end history must end exactly at as_of")
        if self.future_covariates is not None:
            future = [parse_utc_timestamp(value) for value in self.future_covariates.timestamps]
            if any(value <= as_of for value in future):
                raise ValueError("future covariates must be after as_of")
            self._validate_cadence(future)
            if not future or int((future[0] - as_of).total_seconds()) != self.cadence_seconds:
                raise ValueError("future covariates must start one cadence after as_of")
        input_watermark = parse_utc_timestamp(self.quality.input_watermark)
        if input_watermark > as_of:
            raise ValueError("input watermark must not be after as_of")
        expected_provenance = {("history", name) for name in self.history.features}
        if self.future_covariates is not None:
            expected_provenance.update(
                ("future_covariates", name) for name in self.future_covariates.features
            )
        expected_provenance.update(("static_features", name) for name in self.static_features)
        actual_provenance = [(entry.segment, entry.feature) for entry in self.provenance]
        if (
            len(actual_provenance) != len(expected_provenance)
            or set(actual_provenance) != expected_provenance
        ):
            raise ValueError("every frame feature requires exactly one provenance entry")
        actual_watermarks: list[datetime] = []
        for entry in self.provenance:
            allowed_source_kinds = {
                "history": {"history", "live", "history_and_live", "calendar"},
                "future_covariates": {"covariate", "calendar", "constant"},
                "static_features": {"constant"},
            }
            if entry.source_kind not in allowed_source_kinds[entry.segment]:
                raise ValueError("provenance source kind does not match its segment")
            if entry.issued_at is not None and not (
                entry.segment == "future_covariates" and entry.source_kind == "covariate"
            ):
                raise ValueError("issue time is allowed only for versioned future covariates")
            watermark = parse_utc_timestamp(entry.watermark)
            if watermark > as_of:
                raise ValueError("provenance watermark must not be after frame as_of")
            if entry.issued_at is not None:
                issued_at = parse_utc_timestamp(entry.issued_at)
                if issued_at > watermark:
                    raise ValueError("provenance issued_at must not follow its watermark")
            if entry.source_kind not in {"calendar", "constant"}:
                actual_watermarks.append(watermark)
        if not actual_watermarks or max(actual_watermarks) != input_watermark:
            raise ValueError("input watermark must equal the newest actual source watermark")
        return self

    def _validate_cadence(self, timestamps: list[datetime]) -> None:
        if any(
            int((right - left).total_seconds()) != self.cadence_seconds
            for left, right in pairwise(timestamps)
        ):
            raise ValueError("adjacent timestamps must match cadence_seconds")


class TaskRef(ContractModel):
    id: Identifier
    revision: Annotated[StrictInt, Field(gt=0)]
    kind: Literal["forecast"]

    @field_validator("id")
    @classmethod
    def id_is_valid(cls, value: str) -> str:
        if not _IDENTIFIER.fullmatch(value):
            raise ValueError("task id is invalid")
        return value


class BindingRef(ContractModel):
    id: Identifier
    revision: Annotated[StrictInt, Field(gt=0)]

    @field_validator("id")
    @classmethod
    def id_is_valid(cls, value: str) -> str:
        if not _IDENTIFIER.fullmatch(value):
            raise ValueError("binding id is invalid")
        return value


class ArtifactSelector(ContractModel):
    kind: Identifier
    family: Identifier
    version: Identifier | None = None
    artifact_digest: Digest | None = None


class ForecastOptions(ContractModel):
    kind: Literal["forecast"]
    horizon_steps: Annotated[StrictInt, Field(gt=0, le=4096)]
    quantiles: Annotated[list[StrictFloat], Field(min_length=1, max_length=19)] | None = None

    @field_validator("quantiles")
    @classmethod
    def quantiles_are_valid(cls, values: list[float] | None) -> list[float] | None:
        if values is None:
            return values
        if any(not math.isfinite(value) or not 0.0 < value < 1.0 for value in values):
            raise ValueError("quantiles must be finite and inside (0, 1)")
        if any(left >= right for left, right in pairwise(values)):
            raise ValueError("quantiles must be unique and increasing")
        return values


class DataProcessingRequest(ContractModel):
    schema_: Literal[REQUEST_SCHEMA] = Field(alias="schema")
    request_id: Identifier
    submitted_at: Timestamp
    deadline: Timestamp
    task: TaskRef
    binding: BindingRef
    processor_contract: Literal[FORECAST_CONTRACT]
    artifact: ArtifactSelector | None = None
    frame: ProcessingFrame
    options: ForecastOptions
    input_digest: Digest

    @model_validator(mode="before")
    @classmethod
    def explicit_nulls_are_rejected(cls, value: Any) -> Any:
        def visit(item: Any, *, values_array: bool = False) -> None:
            if isinstance(item, dict):
                for name, child in item.items():
                    if child is None and name != "value":
                        raise ValueError("optional contract fields must be omitted, not null")
                    if child is not None:
                        visit(child, values_array=name == "values")
            elif isinstance(item, list):
                for child in item:
                    if child is None and not values_array:
                        raise ValueError(
                            "null is allowed only for explicitly missing feature values"
                        )
                    if child is not None:
                        visit(child)

        visit(value)
        return value

    @field_validator("request_id")
    @classmethod
    def request_id_is_uuid(cls, value: str) -> str:
        try:
            UUID(value)
        except ValueError as exc:
            raise ValueError("request_id must be a UUID") from exc
        return value

    @field_validator("submitted_at", "deadline")
    @classmethod
    def request_time_is_utc(cls, value: str) -> str:
        parse_utc_timestamp(value)
        return value

    @field_validator("input_digest")
    @classmethod
    def input_digest_is_valid(cls, value: str) -> str:
        if not _DIGEST.fullmatch(value):
            raise ValueError("input_digest must be lowercase sha256")
        return value

    @model_validator(mode="after")
    def deadline_follows_submission(self) -> DataProcessingRequest:
        if parse_utc_timestamp(self.deadline) <= parse_utc_timestamp(self.submitted_at):
            raise ValueError("deadline must be after submitted_at")
        return self


def compute_input_digest(request: DataProcessingRequest) -> str:
    frame = request.frame.model_dump(exclude_none=True)
    frame["as_of"] = canonical_digest_timestamp(frame["as_of"])
    frame["quality"]["input_watermark"] = canonical_digest_timestamp(
        frame["quality"]["input_watermark"]
    )
    for segment_name in ("history", "future_covariates"):
        segment = frame.get(segment_name)
        if segment is not None:
            segment["timestamps"] = [
                canonical_digest_timestamp(value) for value in segment["timestamps"]
            ]
    for provenance in frame["provenance"]:
        provenance["watermark"] = canonical_digest_timestamp(provenance["watermark"])
        if "issued_at" in provenance:
            provenance["issued_at"] = canonical_digest_timestamp(provenance["issued_at"])
    digest_input: dict[str, Any] = {
        "task": request.task.model_dump(exclude_none=True),
        "binding": request.binding.model_dump(exclude_none=True),
        "processor_contract": request.processor_contract,
        "artifact": (
            request.artifact.model_dump(exclude_none=True) if request.artifact is not None else None
        ),
        "frame": frame,
        "options": request.options.model_dump(exclude_none=True),
    }
    return f"sha256:{hashlib.sha256(rfc8785.dumps(digest_input)).hexdigest()}"


class ProcessorDescriptor(ContractModel):
    id: Identifier
    version: Identifier
    contract: Literal[FORECAST_CONTRACT]


class ArtifactProvenanceModel(ContractModel):
    kind: Identifier
    family: Identifier
    version: Identifier
    artifact_digest: Digest

    @field_validator("artifact_digest")
    @classmethod
    def artifact_digest_is_valid(cls, value: str) -> str:
        if not _DIGEST.fullmatch(value):
            raise ValueError("artifact_digest must be lowercase sha256")
        return value


class QuantileValue(ContractModel):
    probability: Annotated[float, Field(gt=0.0, lt=1.0, allow_inf_nan=False)]
    value: Annotated[float, Field(allow_inf_nan=False)]


class ForecastPoint(ContractModel):
    timestamp: Timestamp
    value: Annotated[float, Field(allow_inf_nan=False)]
    quantiles: list[QuantileValue] | None = None

    @field_validator("timestamp")
    @classmethod
    def timestamp_is_utc(cls, value: str) -> str:
        parse_utc_timestamp(value)
        return value

    @field_serializer("timestamp")
    def serialize_timestamp(self, value: str) -> str:
        return normalize_utc_timestamp(value)


class ForecastOutput(ContractModel):
    schema_: Literal[FORECAST_OUTPUT_SCHEMA] = Field(alias="schema")
    kind: Literal["forecast"]
    target: Literal["load"]
    unit: Literal["kW"]
    sign_convention: Literal["positive_consumption"]
    cadence_seconds: Annotated[StrictInt, Field(gt=0)]
    timestamp_semantics: Literal["interval_end"]
    points: Annotated[list[ForecastPoint], Field(min_length=1, max_length=4096)]


class FallbackDescriptor(ContractModel):
    strategy: Literal["persistence"]
    strategy_version: Literal["1"]
    reason_code: Literal["MODEL_UNAVAILABLE"]
    source_feature: Literal["load"]
    based_on_data_through: Timestamp

    @field_serializer("based_on_data_through")
    def serialize_data_cut(self, value: str) -> str:
        return normalize_utc_timestamp(value)


class UnavailableDescriptor(ContractModel):
    reason_code: StableCode
    retryable: StrictBool
    retry_after_seconds: Annotated[StrictInt, Field(gt=0)] | None = None


class ProcessingResult(ContractModel):
    schema_: Literal[RESULT_SCHEMA] = Field(alias="schema")
    request_id: Identifier
    task: TaskRef
    binding: BindingRef
    input_digest: Digest
    status: Literal["produced", "fallback", "unavailable"]
    issued_at: Timestamp
    expires_at: Timestamp | None = None
    input_watermark: Timestamp
    processor: ProcessorDescriptor
    artifact: ArtifactProvenanceModel | None = None
    output: ForecastOutput | None = None
    fallback: FallbackDescriptor | None = None
    unavailable: UnavailableDescriptor | None = None
    warnings: Annotated[list[StableCode], Field(max_length=64)]

    @field_serializer("issued_at", "input_watermark")
    def serialize_required_timestamp(self, value: str) -> str:
        return normalize_utc_timestamp(value)

    @field_serializer("expires_at")
    def serialize_optional_timestamp(self, value: str | None) -> str | None:
        return normalize_utc_timestamp(value) if value is not None else None

    @model_validator(mode="after")
    def status_controls_payload(self) -> ProcessingResult:
        parse_utc_timestamp(self.issued_at)
        parse_utc_timestamp(self.input_watermark)
        if self.status == "unavailable":
            if self.unavailable is None or any(
                value is not None for value in (self.output, self.expires_at, self.fallback)
            ):
                raise ValueError("unavailable result has an invalid payload")
        elif self.output is None or self.expires_at is None or self.unavailable is not None:
            raise ValueError("produced and fallback results require output and expiry")
        elif self.status == "fallback" and self.fallback is None:
            raise ValueError("fallback result requires fallback provenance")
        elif self.status == "produced" and self.fallback is not None:
            raise ValueError("produced result must not carry fallback provenance")
        return self


class ProcessingError(ContractModel):
    schema_: Literal[ERROR_SCHEMA] = Field(default=ERROR_SCHEMA, alias="schema")
    request_id: Identifier | None = None
    code: StableCode
    category: Literal[
        "invalid_request",
        "authorization",
        "resource_limit",
        "invalid_data",
        "capacity",
        "internal",
        "timeout",
    ]
    message: ShortText
    retryable: StrictBool
