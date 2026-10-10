"""PyInstaller entry point for the frozen sidecar (Phase 15).

Freezing the `laya-serve` console shim directly bakes a venv shebang path
into the bundle; this indirection keeps the frozen binary relocatable.
`laya.serve.main()` is env-only configured (LAYA_HOST/PORT/MODELS/...) —
see sidecar/CONTRACT.md.
"""

from laya.serve import main

if __name__ == "__main__":
    main()
