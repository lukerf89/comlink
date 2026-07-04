# Audio Fixtures

`short.wav` is generated locally with `scripts/dev/generate-short-fixture.sh`.
On macOS the script uses `say` to synthesize "Comlink phase zero fixture.",
then normalizes it to 16 kHz mono WAV with FFmpeg. On other platforms it falls
back to a short synthetic tone for adapter-level tests.
