#!/usr/bin/env bash
set -euo pipefail

grep -qx "hello from smol eval" message.txt
