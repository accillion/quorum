#!/bin/sh
# Run from the repo root inside python@sha256:4209fae09a3e20afd863e528bfddc235da5fd5977983e1996cd54cc9a72e6267
# (python:3.12-bookworm), with the files under repro/files/ copied in.
set -e
pip install -q -e ".[dev]" && python -m pytest -p no:cacheprovider tests/test_transport.py
