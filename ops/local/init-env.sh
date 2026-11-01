#!/bin/sh
# Creates .env from .env.example. Never overwrites.
set -eu
cd "$(dirname "$0")/../.."
if [ -f .env ]; then echo ".env exists; leaving it alone"; exit 0; fi
cp .env.example .env
chmod 600 .env
echo "created .env"
