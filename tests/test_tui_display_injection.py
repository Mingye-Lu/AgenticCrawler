"""Tests for display parameter injection in run_agent()."""

import inspect
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from agentic_crawler.agent.loop import run_agent
from agentic_crawler.agent.display import AgentDisplay, LiveDashboard
from agentic_crawler.config import Settings


class TestRunAgentDisplayParameter:
    """Test suite for run_agent() display parameter."""

    def test_run_agent_signature_has_display_parameter(self) -> None:
        """Verify run_agent() accepts optional display parameter."""
        sig = inspect.signature(run_agent)
        assert "display" in sig.parameters
        param = sig.parameters["display"]
        assert param.default is None

    @pytest.mark.asyncio
    async def test_run_agent_with_custom_display(self) -> None:
        """run_agent(display=mock_display) uses provided display instance."""
        mock_display = MagicMock(spec=AgentDisplay)
        mock_display.register_agent = MagicMock()

        mock_settings = MagicMock(spec=Settings)
        mock_settings.max_steps = 5
        mock_settings.max_concurrent_per_parent = 3
        mock_settings.max_fork_depth = 2
        mock_settings.max_total_agents = 10

        with (
            patch("agentic_crawler.agent.loop.get_provider") as mock_get_provider,
            patch("agentic_crawler.agent.crawl_agent.CrawlAgent") as MockCrawlAgent,
        ):
            mock_provider = MagicMock()
            mock_provider.close = AsyncMock()
            mock_get_provider.return_value = mock_provider

            mock_agent_instance = AsyncMock()
            MockCrawlAgent.return_value = mock_agent_instance

            await run_agent(goal="test goal", settings=mock_settings, display=mock_display)

            mock_display.register_agent.assert_called_once()
            call_args = mock_display.register_agent.call_args
            assert call_args[0][1] == "test goal"

    @pytest.mark.asyncio
    async def test_run_agent_with_custom_display_does_not_call_start(self) -> None:
        """run_agent(display=mock_display) does NOT call start() on provided display."""
        mock_display = MagicMock(spec=AgentDisplay)
        mock_display.register_agent = MagicMock()

        mock_settings = MagicMock(spec=Settings)
        mock_settings.max_steps = 5
        mock_settings.max_concurrent_per_parent = 3
        mock_settings.max_fork_depth = 2
        mock_settings.max_total_agents = 10

        with (
            patch("agentic_crawler.agent.loop.get_provider") as mock_get_provider,
            patch("agentic_crawler.agent.crawl_agent.CrawlAgent") as MockCrawlAgent,
        ):
            mock_provider = MagicMock()
            mock_provider.close = AsyncMock()
            mock_get_provider.return_value = mock_provider

            mock_agent_instance = AsyncMock()
            MockCrawlAgent.return_value = mock_agent_instance

            await run_agent(goal="test goal", settings=mock_settings, display=mock_display)

            if hasattr(mock_display, "start"):
                mock_display.start.assert_not_called()

    @pytest.mark.asyncio
    async def test_run_agent_with_none_display_creates_livedashboard(self) -> None:
        """run_agent(display=None) creates LiveDashboard and calls start()."""
        mock_settings = MagicMock(spec=Settings)
        mock_settings.max_steps = 5
        mock_settings.max_concurrent_per_parent = 3
        mock_settings.max_fork_depth = 2
        mock_settings.max_total_agents = 10

        with (
            patch("agentic_crawler.agent.loop.get_provider") as mock_get_provider,
            patch("agentic_crawler.agent.crawl_agent.CrawlAgent") as MockCrawlAgent,
            patch("agentic_crawler.agent.loop.LiveDashboard") as MockLiveDashboard,
        ):
            mock_provider = MagicMock()
            mock_provider.close = AsyncMock()
            mock_get_provider.return_value = mock_provider

            mock_dashboard_instance = MagicMock()
            mock_dashboard_instance.register_agent = MagicMock()
            mock_dashboard_instance.start = MagicMock()
            MockLiveDashboard.return_value = mock_dashboard_instance

            mock_agent_instance = AsyncMock()
            MockCrawlAgent.return_value = mock_agent_instance

            await run_agent(goal="test goal", settings=mock_settings, display=None)

            MockLiveDashboard.assert_called_once()
            mock_dashboard_instance.register_agent.assert_called_once()
            mock_dashboard_instance.start.assert_called_once()

    @pytest.mark.asyncio
    async def test_run_agent_default_display_none_creates_livedashboard(self) -> None:
        """run_agent() without display parameter defaults to creating LiveDashboard."""
        mock_settings = MagicMock(spec=Settings)
        mock_settings.max_steps = 5
        mock_settings.max_concurrent_per_parent = 3
        mock_settings.max_fork_depth = 2
        mock_settings.max_total_agents = 10

        with (
            patch("agentic_crawler.agent.loop.get_provider") as mock_get_provider,
            patch("agentic_crawler.agent.crawl_agent.CrawlAgent") as MockCrawlAgent,
            patch("agentic_crawler.agent.loop.LiveDashboard") as MockLiveDashboard,
        ):
            mock_provider = MagicMock()
            mock_provider.close = AsyncMock()
            mock_get_provider.return_value = mock_provider

            mock_dashboard_instance = MagicMock()
            mock_dashboard_instance.register_agent = MagicMock()
            mock_dashboard_instance.start = MagicMock()
            MockLiveDashboard.return_value = mock_dashboard_instance

            mock_agent_instance = AsyncMock()
            MockCrawlAgent.return_value = mock_agent_instance

            await run_agent(goal="test goal", settings=mock_settings)

            MockLiveDashboard.assert_called_once()
            mock_dashboard_instance.register_agent.assert_called_once()
            mock_dashboard_instance.start.assert_called_once()
