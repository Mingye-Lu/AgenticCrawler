"""Test suite for Typer CLI removal and prompt-toolkit integration.

This test file verifies:
1. Typer dependency is removed from pyproject.toml
2. No typer imports exist in src/agentic_crawler/
3. agentic_crawler.cli module no longer exists
4. agentic_crawler.__main__ module exists and is importable
5. main() function is callable from __main__
"""

import importlib
import sys
from pathlib import Path


def test_typer_not_in_dependencies():
    """Verify typer is NOT in pyproject.toml dependencies."""
    pyproject_path = Path(__file__).parent.parent / "pyproject.toml"
    content = pyproject_path.read_text()

    # Check that typer is not in the dependencies list
    assert "typer>=" not in content, "typer dependency should be removed from pyproject.toml"
    assert '"typer' not in content, "typer should not appear in pyproject.toml"


def test_prompt_toolkit_in_dependencies():
    """Verify prompt-toolkit is in pyproject.toml dependencies."""
    pyproject_path = Path(__file__).parent.parent / "pyproject.toml"
    content = pyproject_path.read_text()

    # Check that prompt-toolkit is in the dependencies list
    assert "prompt-toolkit>=" in content, "prompt-toolkit should be added to pyproject.toml"


def test_no_typer_imports_in_src():
    """Verify no typer imports exist in src/agentic_crawler/."""
    src_path = Path(__file__).parent.parent / "src" / "agentic_crawler"

    typer_imports = []
    for py_file in src_path.rglob("*.py"):
        try:
            content = py_file.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            content = py_file.read_text(encoding="gbk", errors="ignore")
        if "import typer" in content or "from typer" in content:
            typer_imports.append(str(py_file))

    assert not typer_imports, f"Found typer imports in: {typer_imports}"


def test_cli_module_does_not_exist():
    """Verify agentic_crawler.cli module no longer exists."""
    # Remove from sys.modules if it was previously imported
    if "agentic_crawler.cli" in sys.modules:
        del sys.modules["agentic_crawler.cli"]

    # Try to import and verify it fails
    try:
        importlib.import_module("agentic_crawler.cli")
        assert False, "agentic_crawler.cli should not be importable"
    except (ImportError, ModuleNotFoundError):
        # Expected behavior
        pass


def test_main_module_imports_successfully():
    """Verify agentic_crawler.__main__ module imports successfully."""
    # Remove from sys.modules if it was previously imported
    if "agentic_crawler.__main__" in sys.modules:
        del sys.modules["agentic_crawler.__main__"]

    # Should import without errors
    main_module = importlib.import_module("agentic_crawler.__main__")
    assert main_module is not None


def test_main_function_exists_and_callable():
    """Verify main() function exists in __main__ and is callable."""
    # Remove from sys.modules if it was previously imported
    if "agentic_crawler.__main__" in sys.modules:
        del sys.modules["agentic_crawler.__main__"]

    from agentic_crawler.__main__ import main

    assert callable(main), "main should be a callable function"


def test_scripts_entry_point_updated():
    """Verify pyproject.toml scripts entry point is updated."""
    pyproject_path = Path(__file__).parent.parent / "pyproject.toml"
    content = pyproject_path.read_text()

    # Check that the entry point is updated
    assert 'agentic-crawler = "agentic_crawler.__main__:main"' in content, (
        "scripts entry point should be updated to agentic_crawler.__main__:main"
    )

    # Verify old entry point is gone
    assert "agentic_crawler.cli:app" not in content, "old cli:app entry point should be removed"
