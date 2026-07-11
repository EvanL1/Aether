from __future__ import annotations

import json
from pathlib import Path

from aether_load_forecasting_processor.models import (
    DataProcessingRequest,
    compute_input_digest,
)
from aether_load_forecasting_processor.processor import ProcessorPolicy

REPOSITORY_ROOT = Path(__file__).resolve().parents[3]


def test_ems_fixture_uses_the_same_rfc8785_digest() -> None:
    fixture = json.loads(
        (
            REPOSITORY_ROOT / "packs/energy/data-processing/fixtures/load-processing-request.json"
        ).read_text(encoding="utf-8")
    )

    request = DataProcessingRequest.model_validate(fixture)

    assert compute_input_digest(request) == fixture["input_digest"]


def test_ems_result_fixture_uses_the_default_processor_identity() -> None:
    fixture = json.loads(
        (
            REPOSITORY_ROOT / "packs/energy/data-processing/fixtures/load-processing-result.json"
        ).read_text(encoding="utf-8")
    )

    assert fixture["processor"]["id"] == ProcessorPolicy().processor_id
