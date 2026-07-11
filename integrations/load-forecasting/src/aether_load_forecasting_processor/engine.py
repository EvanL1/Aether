"""Forecast engine boundary and the legacy Edge-Platform in-memory adapter."""

from __future__ import annotations

import hashlib
import hmac
import json
import math
import os
import re
import stat
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from types import MappingProxyType
from typing import Any, ClassVar, Protocol

_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
_BUNDLE_FILE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,255}$")
ARTIFACT_BUNDLES_ENV = "AETHER_LOAD_FORECASTING_ARTIFACT_BUNDLES"
_BUNDLE_DOMAIN = b"aether.artifact.bundle.v1\x00"


class EngineUnavailable(RuntimeError):
    """The injected model engine cannot produce an approved forecast."""


@dataclass(frozen=True, slots=True)
class _FileFingerprint:
    device: int
    inode: int
    size: int
    modified_ns: int


def _normalize_bundle_files(files: Mapping[str, str | Path]) -> dict[str, Path]:
    if not files or len(files) > 64:
        raise ValueError("artifact bundle must declare between 1 and 64 files")
    normalized: dict[str, Path] = {}
    resolved_paths: set[Path] = set()
    for logical_name, configured_path in files.items():
        if (
            not isinstance(logical_name, str)
            or not _BUNDLE_FILE_NAME.fullmatch(logical_name)
            or ".." in Path(logical_name).parts
        ):
            raise ValueError("artifact bundle logical file name is invalid")
        if not isinstance(configured_path, (str, Path)):
            raise ValueError("artifact bundle path is invalid")
        path = Path(configured_path)
        if not path.is_absolute():
            raise ValueError("artifact bundle paths must be absolute")
        if path.is_symlink():
            raise ValueError("artifact bundle paths must not be symbolic links")
        try:
            metadata = path.stat()
        except OSError as exc:
            raise ValueError("artifact bundle file is unavailable") from exc
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError("artifact bundle paths must name regular files")
        resolved = path.resolve(strict=True)
        if resolved in resolved_paths:
            raise ValueError("artifact bundle paths must be unique")
        resolved_paths.add(resolved)
        normalized[logical_name] = path
    return normalized


def _hash_bundle_files(
    files: Mapping[str, Path],
) -> tuple[str, dict[str, _FileFingerprint]]:
    hasher = hashlib.sha256()
    hasher.update(_BUNDLE_DOMAIN)
    fingerprints: dict[str, _FileFingerprint] = {}
    for logical_name in sorted(files):
        path = files[logical_name]
        encoded_name = logical_name.encode("utf-8")
        hasher.update(len(encoded_name).to_bytes(4, "big"))
        hasher.update(encoded_name)
        try:
            with path.open("rb") as artifact_file:
                before = os.fstat(artifact_file.fileno())
                hasher.update(before.st_size.to_bytes(8, "big"))
                while chunk := artifact_file.read(1024 * 1024):
                    hasher.update(chunk)
                after = os.fstat(artifact_file.fileno())
        except OSError as exc:
            raise ValueError("artifact bundle file could not be hashed") from exc
        before_identity = (
            before.st_dev,
            before.st_ino,
            before.st_size,
            before.st_mtime_ns,
        )
        after_identity = (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_mtime_ns,
        )
        if before_identity != after_identity:
            raise ValueError("artifact bundle file changed while it was hashed")
        fingerprints[logical_name] = _FileFingerprint(*after_identity)
    return f"sha256:{hasher.hexdigest()}", fingerprints


def compute_artifact_bundle_digest(files: Mapping[str, str | Path]) -> str:
    """Hash logical file names, sizes, and bytes using the bundle-v1 domain."""

    normalized = _normalize_bundle_files(files)
    digest, _fingerprints = _hash_bundle_files(normalized)
    return digest


@dataclass(frozen=True, slots=True)
class CommissionedArtifactBundle:
    """A locally verified immutable set of files used by one model version."""

    files: Mapping[str, Path | str]
    expected_digest: str
    actual_digest: str = field(init=False)
    _fingerprints: Mapping[str, _FileFingerprint] = field(init=False, repr=False)

    def __post_init__(self) -> None:
        if not _DIGEST.fullmatch(self.expected_digest):
            raise ValueError("commissioned artifact bundle digest is invalid")
        normalized = _normalize_bundle_files(self.files)
        actual_digest, fingerprints = _hash_bundle_files(normalized)
        if not hmac.compare_digest(actual_digest, self.expected_digest):
            raise ValueError("commissioned artifact bundle digest does not match its files")
        object.__setattr__(self, "files", MappingProxyType(normalized))
        object.__setattr__(self, "actual_digest", actual_digest)
        object.__setattr__(self, "_fingerprints", MappingProxyType(fingerprints))

    def verify_unchanged(self) -> None:
        """Fail before inference if a commissioned path changed after startup."""

        fingerprints = self._fingerprints
        for logical_name, path in self.files.items():
            try:
                if path.is_symlink():
                    raise EngineUnavailable("artifact bundle changed after commissioning")
                metadata = path.stat()
            except OSError:
                raise EngineUnavailable("artifact bundle changed after commissioning") from None
            current = _FileFingerprint(
                metadata.st_dev,
                metadata.st_ino,
                metadata.st_size,
                metadata.st_mtime_ns,
            )
            if current != fingerprints[logical_name]:
                raise EngineUnavailable("artifact bundle changed after commissioning")


def load_commissioned_artifact_bundles_from_env(
    *,
    required: bool = True,
) -> dict[tuple[str, str, str], CommissionedArtifactBundle]:
    """Load and hash strict commissioned bundle declarations from one JSON env value."""

    raw = os.getenv(ARTIFACT_BUNDLES_ENV)
    if not raw:
        if required:
            raise ValueError(f"{ARTIFACT_BUNDLES_ENV} is required")
        return {}
    try:
        declarations = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ValueError(f"{ARTIFACT_BUNDLES_ENV} must contain valid JSON") from exc
    if not isinstance(declarations, list) or not declarations or len(declarations) > 128:
        raise ValueError(f"{ARTIFACT_BUNDLES_ENV} must be a non-empty JSON array")
    expected_fields = {"kind", "family", "version", "expected_digest", "files"}
    bundles: dict[tuple[str, str, str], CommissionedArtifactBundle] = {}
    for declaration in declarations:
        if not isinstance(declaration, dict) or set(declaration) != expected_fields:
            raise ValueError("artifact bundle declaration has invalid fields")
        kind = declaration["kind"]
        family = declaration["family"]
        version = declaration["version"]
        digest = declaration["expected_digest"]
        files = declaration["files"]
        if (
            not all(isinstance(value, str) and value.strip() for value in (kind, family, version))
            or not isinstance(digest, str)
            or not isinstance(files, dict)
            or any(
                not isinstance(name, str) or not isinstance(path, str)
                for name, path in files.items()
            )
        ):
            raise ValueError("artifact bundle declaration has invalid values")
        key = (kind, family, version)
        if key in bundles:
            raise ValueError("artifact bundle selection is duplicated")
        bundles[key] = CommissionedArtifactBundle(files=files, expected_digest=digest)
    return bundles


@dataclass(frozen=True, slots=True)
class EngineQuantile:
    probability: float
    value: float


@dataclass(frozen=True, slots=True)
class EngineForecastPoint:
    timestamp: str
    value: float
    quantiles: tuple[EngineQuantile, ...] = ()


@dataclass(frozen=True, slots=True)
class ArtifactProvenance:
    kind: str
    family: str
    version: str
    artifact_digest: str


@dataclass(frozen=True, slots=True)
class EngineForecast:
    points: tuple[EngineForecastPoint, ...]
    artifact: ArtifactProvenance | None = None


@dataclass(frozen=True, slots=True)
class ForecastContext:
    request_id: str
    binding_id: str
    as_of: str
    cadence_seconds: int
    horizon_steps: int
    artifact_kind: str | None
    artifact_family: str | None
    artifact_version: str | None
    quantiles: tuple[float, ...]


class ForecastEngine(Protocol):
    """Injected model boundary; it receives data, never Aether read handles."""

    def forecast(
        self,
        *,
        history_data: list[dict[str, Any]],
        forecast_data: list[dict[str, Any]],
        context: ForecastContext,
    ) -> EngineForecast: ...


class InferenceServiceLike(Protocol):
    """Structural type implemented by the existing Edge-Platform service."""

    def run_inference(
        self, pre_out: dict[str, Any], raw_event: dict[str, Any]
    ) -> Mapping[str, Any]: ...


class EdgePlatformInferenceServiceEngine:
    """Call the old InferenceService with complete request-provided rows only."""

    _DEFAULT_HORIZONS: ClassVar[dict[tuple[int, int], str]] = {
        (900, 16): "ultra_short",
        (900, 288): "short_term",
        (900, 960): "medium_term",
    }

    def __init__(
        self,
        inference_service: InferenceServiceLike,
        horizon_names: Mapping[tuple[int, int], str] | None = None,
        artifact_bundles: Mapping[tuple[str, str, str], CommissionedArtifactBundle] | None = None,
        artifact_file_resolver: Callable[[ForecastContext], Mapping[str, str | Path]] | None = None,
        readiness_probe: Callable[[], bool] | None = None,
    ) -> None:
        self._inference_service = inference_service
        self._horizon_names = dict(self._DEFAULT_HORIZONS)
        if horizon_names is not None:
            self._horizon_names.update(horizon_names)
        self._artifact_bundles = dict(artifact_bundles or {})
        self._artifact_file_resolver = artifact_file_resolver
        self._readiness_probe = readiness_probe
        if any(
            len(key) != 3
            or not all(isinstance(component, str) and component.strip() for component in key)
            or not isinstance(bundle, CommissionedArtifactBundle)
            for key, bundle in self._artifact_bundles.items()
        ):
            raise ValueError("artifact bundle policy is invalid")
        if artifact_file_resolver is not None and not callable(artifact_file_resolver):
            raise ValueError("artifact file resolver must be callable")
        if readiness_probe is not None and not callable(readiness_probe):
            raise ValueError("readiness probe must be callable")

    def is_ready(self) -> bool:
        """Report whether artifact proof exists and local files remain commissioned."""

        if (
            not self._artifact_bundles
            or self._artifact_file_resolver is None
            or self._readiness_probe is None
        ):
            return False
        try:
            for bundle in self._artifact_bundles.values():
                bundle.verify_unchanged()
            return self._readiness_probe() is True
        except Exception:
            return False

    def forecast(
        self,
        *,
        history_data: list[dict[str, Any]],
        forecast_data: list[dict[str, Any]],
        context: ForecastContext,
    ) -> EngineForecast:
        if context.artifact_version is None:
            raise EngineUnavailable("an explicit model version is required")
        if context.quantiles:
            raise EngineUnavailable("the legacy engine does not support quantiles")
        if context.artifact_kind is None or context.artifact_family is None:
            raise EngineUnavailable("an explicit model selector is required")
        artifact_key = (
            context.artifact_kind,
            context.artifact_family,
            context.artifact_version,
        )
        bundle = self._artifact_bundles.get(artifact_key)
        if bundle is None:
            raise EngineUnavailable("requested artifact bundle is not commissioned")
        bundle.verify_unchanged()

        horizon = self._horizon_names.get(
            (context.cadence_seconds, context.horizon_steps), "custom"
        )
        pre_out = {
            "ok": True,
            "plant_id": context.binding_id,
            "forecast_type": "load_forecast",
            "horizon": horizon,
            "as_of": context.as_of,
            "data": {"history": history_data, "forecast": forecast_data},
        }
        raw_event = {
            "plant_id": context.binding_id,
            "forecast_type": "load_forecast",
            "horizon": horizon,
            "as_of": context.as_of,
            "model_version": context.artifact_version,
        }
        self._verify_resolved_artifact_files(bundle, context)
        self._verify_legacy_readiness()

        try:
            response = self._inference_service.run_inference(pre_out, raw_event)
            body = self._parse_body(response)
            if body.get("model_version") != context.artifact_version:
                raise EngineUnavailable("edge platform model version does not match request")
            note = body.get("note")
            if isinstance(note, str) and "BASELINE_FALLBACK" in note.upper():
                raise EngineUnavailable("legacy fallback response rejected")
            points = self._parse_points(body.get("predictions"))
            artifact = self._parse_artifact(body, context, bundle)
        except EngineUnavailable:
            raise
        except Exception:
            raise EngineUnavailable("edge platform inference failed") from None
        return EngineForecast(points=points, artifact=artifact)

    def _verify_resolved_artifact_files(
        self,
        bundle: CommissionedArtifactBundle,
        context: ForecastContext,
    ) -> None:
        resolver = self._artifact_file_resolver
        if resolver is None:
            raise EngineUnavailable("legacy artifact path resolution is not commissioned")
        try:
            resolved_files = _normalize_bundle_files(resolver(context))
        except Exception:
            raise EngineUnavailable("legacy artifact path resolution failed") from None
        if set(resolved_files) != set(bundle.files) or any(
            resolved_files[name].resolve(strict=True) != bundle.files[name].resolve(strict=True)
            for name in bundle.files
        ):
            raise EngineUnavailable("legacy artifact paths do not match commissioned bundle")

    def _verify_legacy_readiness(self) -> None:
        probe = self._readiness_probe
        if probe is None:
            raise EngineUnavailable("legacy readiness probe is not commissioned")
        try:
            ready = probe()
        except Exception:
            raise EngineUnavailable("legacy readiness probe failed") from None
        if ready is not True:
            raise EngineUnavailable("legacy inference service is not ready")

    @staticmethod
    def _parse_body(response: Mapping[str, Any]) -> Mapping[str, Any]:
        if response.get("statusCode") != 200:
            raise EngineUnavailable("edge platform inference failed")
        body: Any = response.get("body")
        if isinstance(body, str):
            try:
                body = json.loads(body)
            except json.JSONDecodeError:
                raise EngineUnavailable("edge platform response is not JSON") from None
        if not isinstance(body, Mapping):
            raise EngineUnavailable("edge platform response body is invalid")
        return body

    @staticmethod
    def _parse_points(value: Any) -> tuple[EngineForecastPoint, ...]:
        if not isinstance(value, Sequence) or isinstance(value, (str, bytes)) or not value:
            raise EngineUnavailable("edge platform predictions are missing")
        points: list[EngineForecastPoint] = []
        for item in value:
            if not isinstance(item, Mapping):
                raise EngineUnavailable("edge platform prediction is invalid")
            timestamp = item.get("ts")
            point_value = item.get("value")
            if not isinstance(timestamp, str):
                raise EngineUnavailable("edge platform prediction timestamp is invalid")
            if isinstance(point_value, bool) or not isinstance(point_value, (int, float)):
                raise EngineUnavailable("edge platform prediction value is invalid")
            if not math.isfinite(point_value):
                raise EngineUnavailable("edge platform prediction value is invalid")
            points.append(EngineForecastPoint(timestamp=timestamp, value=float(point_value)))
        return tuple(points)

    @staticmethod
    def _parse_artifact(
        body: Mapping[str, Any],
        context: ForecastContext,
        bundle: CommissionedArtifactBundle,
    ) -> ArtifactProvenance:
        if (
            context.artifact_kind is None
            or context.artifact_family is None
            or context.artifact_version is None
        ):
            raise EngineUnavailable("an explicit model selector is required")
        reported_digest = body.get("artifact_digest")
        if reported_digest is not None and (
            not isinstance(reported_digest, str) or not _DIGEST.fullmatch(reported_digest)
        ):
            raise EngineUnavailable("edge platform artifact digest is invalid")
        if reported_digest is not None and not hmac.compare_digest(
            bundle.actual_digest, reported_digest
        ):
            raise EngineUnavailable("edge platform artifact digest does not match local bundle")
        return ArtifactProvenance(
            kind=context.artifact_kind,
            family=context.artifact_family,
            version=context.artifact_version,
            artifact_digest=bundle.actual_digest,
        )
