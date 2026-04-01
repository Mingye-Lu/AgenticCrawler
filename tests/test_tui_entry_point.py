from __future__ import annotations

import pytest
from unittest.mock import patch
from agentic_crawler.__main__ import main, _build_parser


def test_main_is_callable():
    assert callable(main)


def test_help_flag(capsys):
    with pytest.raises(SystemExit) as exc_info:
        main(["--help"])
    assert exc_info.value.code == 0
    captured = capsys.readouterr()
    assert "agentic-crawler" in captured.out.lower() or "usage" in captured.out.lower()


def test_login_subcommand_invokes_run_login():
    with patch("agentic_crawler.__main__._run_login") as mock_login:
        main(["login"])
        mock_login.assert_called_once()


def test_no_args_launches_repl():
    with patch("agentic_crawler.__main__._run_repl") as mock_repl:
        main([])
        mock_repl.assert_called_once()


def test_login_help(capsys):
    with pytest.raises(SystemExit) as exc_info:
        main(["login", "--help"])
    assert exc_info.value.code == 0
    captured = capsys.readouterr()
    assert any(kw in captured.out.lower() for kw in ("oauth", "login", "authenticate", "codex"))


def test_build_parser_returns_argumentparser():
    import argparse

    parser = _build_parser()
    assert isinstance(parser, argparse.ArgumentParser)


def test_provider_arg_passed_to_repl():
    with patch("agentic_crawler.__main__._run_repl") as mock_repl:
        main(["--provider", "openai"])
        mock_repl.assert_called_once()
        args = mock_repl.call_args[0][0]
        assert args.provider == "openai"


def test_no_headless_flag_set():
    with patch("agentic_crawler.__main__._run_repl") as mock_repl:
        main(["--no-headless"])
        args = mock_repl.call_args[0][0]
        assert args.no_headless is True


def test_verbose_flag_set():
    with patch("agentic_crawler.__main__._run_repl") as mock_repl:
        main(["--verbose"])
        args = mock_repl.call_args[0][0]
        assert args.verbose is True
