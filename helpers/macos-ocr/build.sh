#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
swiftc -O ocr.swift -o ../../target/aloud-ocr
echo "built target/aloud-ocr"
