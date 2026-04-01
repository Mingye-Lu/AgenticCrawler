from __future__ import annotations

import argparse
import pytest
from unittest.mock import patch


def _get_main():
    """Always get fresh reference to main from current module."""
    import agentic_crawler.__main__ as m

    return m.main


def _get_build_parser():
    """Always get fresh reference to _build_parser from current module."""
    import agentic_crawler.__main__ as m

    return m._build_parser


def test_main_is_callable():
    main = _get_main()
    assert callable(main)


def test_help_flag(capsys):
    main = _get_main()
    with pytest.raises(SystemExit) as exc_info:
        main(["--help"])
    assert exc_info.value.code == 0
    captured = capsys.readouterr()
    assert "agentic-crawler" in captured.out.lower() or "usage" in captured.out.lower()


def test_login_subcommand_invokes_run_login():
    import agentic_crawler.__main__ as main_module

    with patch.object(main_module, "_run_login") as mock_login:
        main_module.main(["login"])
        mock_login.assert_called_once()


def test_no_args_launches_repl():
    import agentic_crawler.__main__ as main_module

    with patch.object(main_module, "_run_repl") as mock_repl:
        main_module.main([])
        mock_repl.assert_called_once()


def test_login_help(capsys):
    main = _get_main()
    with pytest.raises(SystemExit) as exc_info:
        main(["login", "--help"])
    assert exc_info.value.code == 0
    captured = capsys.readouterr()
    assert any(kw in captured.out.lower() for kw in ("oauth", "login", "authenticate", "codex"))


def test_build_parser_returns_argumentparser():
    _build_parser = _get_build_parser()
    parser = _build_parser()
    assert isinstance(parser, argparse.ArgumentParser)


def test_provider_arg_passed_to_repl():
    import agentic_crawler.__main__ as main_module

    with patch.object(main_module, "_run_repl") as mock_repl:
        main_module.main(["--provider", "openai"])
        mock_repl.assert_called_once()
        args = mock_repl.call_args[0][0]
        assert args.provider == "openai"


def test_no_headless_flag_set():
    import agentic_crawler.__main__ as main_module

    with patch.object(main_module, "_run_repl") as mock_repl:
        main_module.main(["--no-headless"])
        args = mock_repl.call_args[0][0]
        assert args.no_headless is True


def test_verbose_flag_set():
    import agentic_crawler.__main__ as main_module

    with patch.object(main_module, "_run_repl") as mock_repl:
        main_module.main(["--verbose"])
        args = mock_repl.call_args[0][0]
        assert args.verbose is True
