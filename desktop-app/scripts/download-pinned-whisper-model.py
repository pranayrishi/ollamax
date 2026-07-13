#!/usr/bin/env python3
"""Fetch the CI-only Whisper model through Hugging Face's Xet-aware client.

This helper is deliberately used only by the tagged desktop release workflow.
The checked-in app manifest remains unbundled, and stage-whisper-runtime.mjs
independently rechecks the same file before an installer is produced.
"""

from __future__ import annotations

import os
import shutil
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path

MODEL_FILE = "ggml-base.en.bin"


def required_environment(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise RuntimeError(f"missing required environment variable: {name}")
    return value


def assert_package_version(distribution: str, expected: str) -> None:
    try:
        actual = version(distribution)
    except PackageNotFoundError as error:
        raise RuntimeError(f"required model-download package is missing: {distribution}") from error
    if actual != expected:
        raise RuntimeError(f"{distribution} version must be {expected}, got {actual}")


def main() -> None:
    runner_temp = Path(required_environment("RUNNER_TEMP")).resolve()
    model_dir = runner_temp / "ollamax-whisper-model"
    hf_home = runner_temp / "ollamax-huggingface"
    os.environ["HF_HOME"] = str(hf_home)

    try:
        from huggingface_hub import hf_hub_download
        import hf_xet  # noqa: F401 -- fail clearly if the pinned Xet wheel is unavailable.
    except ImportError as error:
        raise RuntimeError(
            "the pinned huggingface_hub and hf_xet packages must be installed before downloading the Whisper model"
        ) from error
    assert_package_version("huggingface_hub", required_environment("WHISPER_HUB_VERSION"))
    assert_package_version("hf-xet", required_environment("WHISPER_XET_VERSION"))

    # The path is derived from RUNNER_TEMP rather than any user-controlled
    # argument, making this cleanup safe on every hosted platform.
    shutil.rmtree(model_dir, ignore_errors=True)
    model_dir.mkdir(parents=True, exist_ok=True)

    model = Path(
        hf_hub_download(
            repo_id=required_environment("WHISPER_MODEL_REPO"),
            filename=MODEL_FILE,
            revision=required_environment("WHISPER_MODEL_REVISION"),
            local_dir=model_dir,
            force_download=True,
            token=False,
        )
    )
    expected_path = model_dir / MODEL_FILE
    if model.resolve() != expected_path.resolve():
        raise RuntimeError(f"unexpected downloaded Whisper model path: {model}")
    if not model.is_file() or model.stat().st_size == 0:
        raise RuntimeError("downloaded Whisper model is missing or empty")

    github_env = os.environ.get("GITHUB_ENV")
    if github_env:
        with Path(github_env).open("a", encoding="utf-8") as handle:
            handle.write(f"OLLAMAX_WHISPER_MODEL_PATH={model}\n")
    print(f"downloaded reviewed Whisper model: {model}")


if __name__ == "__main__":
    main()
