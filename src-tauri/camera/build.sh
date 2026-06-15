#!/bin/sh
# Compile the camera-capture helper into src-tauri/binaries/.
# Run from anywhere; paths are relative to this script.
set -e
DIR="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$DIR/../binaries"
swiftc -O "$DIR/main.swift" -o "$DIR/../binaries/meetclaw-camera"
echo "built $DIR/../binaries/meetclaw-camera"
