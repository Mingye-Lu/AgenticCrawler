use std::fmt::{Display, Formatter};

/// 16-category failure taxonomy (Skyvern-derived, keyword matching only — zero LLM cost).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureCategory {
    /// Element doesn't exist in DOM
    SelectorNotFound,
    /// Multiple matches
    SelectorAmbiguous,
    /// Exists but hidden/off-screen
    ElementNotVisible,
    /// Exists but disabled
    ElementDisabled,
    /// Page didn't load
    NavigationTimeout,
    /// 403/429/503
    NavigationBlocked,
    /// Bridge disconnected
    BrowserCrash,
    /// `execute_js` failed
    JavaScriptError,
    /// Form rejected input
    FormValidation,
    /// Redirect to login
    AuthRequired,
    /// Bot challenge
    CaptchaDetected,
    /// Too many requests
    RateLimited,
    /// Connection failed
    NetworkError,
    /// Expected content not found
    ContentMismatch,
    /// Cost limit hit
    BudgetExceeded,
    /// Unclassifiable
    Unknown,
}

impl Display for FailureCategory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::SelectorNotFound => "SelectorNotFound",
            Self::SelectorAmbiguous => "SelectorAmbiguous",
            Self::ElementNotVisible => "ElementNotVisible",
            Self::ElementDisabled => "ElementDisabled",
            Self::NavigationTimeout => "NavigationTimeout",
            Self::NavigationBlocked => "NavigationBlocked",
            Self::BrowserCrash => "BrowserCrash",
            Self::JavaScriptError => "JavaScriptError",
            Self::FormValidation => "FormValidation",
            Self::AuthRequired => "AuthRequired",
            Self::CaptchaDetected => "CaptchaDetected",
            Self::RateLimited => "RateLimited",
            Self::NetworkError => "NetworkError",
            Self::ContentMismatch => "ContentMismatch",
            Self::BudgetExceeded => "BudgetExceeded",
            Self::Unknown => "Unknown",
        };
        write!(f, "{name}")
    }
}

/// Retry strategy associated with a failure category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryStrategy {
    /// Try self-healing selector
    RetryWithHealing,
    /// Delay in seconds before retry
    RetryWithDelay(u64),
    /// Reset browser state then retry
    ResetAndRetry,
    /// Do not retry
    NoRetry,
}

/// Classify an error message into a failure category using keyword matching.
/// This is zero-cost — no LLM calls.
#[must_use]
pub fn classify(tool_name: &str, error_message: &str) -> FailureCategory {
    let msg = error_message.to_lowercase();
    let tool = tool_name.to_lowercase();

    // Budget exceeded (check first — explicit internal error)
    if msg.contains("budget exceeded") || msg.contains("cost limit") {
        return FailureCategory::BudgetExceeded;
    }

    // Browser crash
    if msg.contains("bridge")
        && (msg.contains("disconnect") || msg.contains("crash") || msg.contains("closed"))
    {
        return FailureCategory::BrowserCrash;
    }
    if msg.contains("browser closed") || msg.contains("target closed") {
        return FailureCategory::BrowserCrash;
    }

    // CAPTCHA / bot challenge
    if msg.contains("captcha")
        || msg.contains("turnstile")
        || msg.contains("hcaptcha")
        || msg.contains("recaptcha")
        || (msg.contains("challenge") && msg.contains("bot"))
    {
        return FailureCategory::CaptchaDetected;
    }

    // Auth required
    if msg.contains("login")
        || msg.contains("sign in")
        || msg.contains("unauthorized")
        || msg.contains("authentication required")
        || msg.contains("not authenticated")
    {
        return FailureCategory::AuthRequired;
    }

    // Rate limited
    if msg.contains("429") || msg.contains("rate limit") || msg.contains("too many requests") {
        return FailureCategory::RateLimited;
    }

    // Navigation blocked (403/503)
    if msg.contains("403")
        || msg.contains("503")
        || msg.contains("forbidden")
        || msg.contains("access denied")
        || msg.contains("blocked")
    {
        return FailureCategory::NavigationBlocked;
    }

    // Navigation timeout
    if msg.contains("timeout") || msg.contains("timed out") || msg.contains("navigation timeout") {
        return FailureCategory::NavigationTimeout;
    }

    // Selector ambiguous
    if msg.contains("multiple") || msg.contains("ambiguous") || msg.contains("more than one") {
        return FailureCategory::SelectorAmbiguous;
    }

    // Element not visible
    if msg.contains("hidden")
        || msg.contains("not visible")
        || msg.contains("off-screen")
        || msg.contains("not in viewport")
    {
        return FailureCategory::ElementNotVisible;
    }

    // Element disabled
    if msg.contains("disabled") {
        return FailureCategory::ElementDisabled;
    }

    // Content mismatch (check before selector-not-found — both contain "not found")
    if (msg.contains("not found") || msg.contains("expected content missing"))
        && (tool == "read_content" || msg.contains("content"))
    {
        return FailureCategory::ContentMismatch;
    }

    // Selector not found (check after ambiguous/visible/disabled/content-mismatch)
    if msg.contains("not found")
        || msg.contains("no element")
        || msg.contains("element not found")
        || msg.contains("could not find")
        || msg.contains("does not exist")
    {
        return FailureCategory::SelectorNotFound;
    }

    // JavaScript error
    if tool == "execute_js"
        || msg.contains("javascript")
        || msg.contains("script error")
        || msg.contains("uncaught")
        || msg.contains("syntaxerror")
        || msg.contains("referenceerror")
    {
        return FailureCategory::JavaScriptError;
    }

    // Form validation
    if msg.contains("validation")
        || msg.contains("invalid input")
        || msg.contains("required field")
        || msg.contains("form error")
    {
        return FailureCategory::FormValidation;
    }

    // Network error
    if msg.contains("network")
        || msg.contains("connection")
        || msg.contains("dns")
        || msg.contains("unreachable")
        || msg.contains("eof")
        || msg.contains("connection refused")
    {
        return FailureCategory::NetworkError;
    }

    FailureCategory::Unknown
}

/// Get the appropriate retry strategy for a failure category.
#[must_use]
pub fn retry_strategy(category: &FailureCategory) -> RetryStrategy {
    match category {
        FailureCategory::SelectorNotFound | FailureCategory::SelectorAmbiguous => {
            RetryStrategy::RetryWithHealing
        }
        FailureCategory::ElementNotVisible | FailureCategory::ElementDisabled => {
            RetryStrategy::RetryWithDelay(1)
        }
        FailureCategory::NavigationTimeout => RetryStrategy::RetryWithDelay(2),
        FailureCategory::RateLimited => RetryStrategy::RetryWithDelay(5),
        FailureCategory::BrowserCrash => RetryStrategy::ResetAndRetry,
        FailureCategory::CaptchaDetected
        | FailureCategory::BudgetExceeded
        | FailureCategory::AuthRequired
        | FailureCategory::NavigationBlocked
        | FailureCategory::JavaScriptError
        | FailureCategory::FormValidation
        | FailureCategory::NetworkError
        | FailureCategory::ContentMismatch
        | FailureCategory::Unknown => RetryStrategy::NoRetry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_selector_not_found() {
        assert_eq!(
            classify("click", "Element not found matching selector @e5"),
            FailureCategory::SelectorNotFound
        );
        assert_eq!(
            classify("click", "could not find element"),
            FailureCategory::SelectorNotFound
        );
    }

    #[test]
    fn classify_selector_ambiguous() {
        assert_eq!(
            classify("click", "Found multiple matches for selector"),
            FailureCategory::SelectorAmbiguous
        );
        assert_eq!(
            classify("click", "Ambiguous selector — more than one match"),
            FailureCategory::SelectorAmbiguous
        );
    }

    #[test]
    fn classify_element_not_visible() {
        assert_eq!(
            classify("click", "Element is hidden behind overlay"),
            FailureCategory::ElementNotVisible
        );
        assert_eq!(
            classify("click", "Element not visible in current viewport"),
            FailureCategory::ElementNotVisible
        );
    }

    #[test]
    fn classify_element_disabled() {
        assert_eq!(
            classify("fill_form", "Element is disabled"),
            FailureCategory::ElementDisabled
        );
    }

    #[test]
    fn classify_navigation_timeout() {
        assert_eq!(
            classify("navigate", "Navigation timed out after 30 seconds"),
            FailureCategory::NavigationTimeout
        );
        assert_eq!(
            classify("navigate", "Page load timeout exceeded"),
            FailureCategory::NavigationTimeout
        );
    }

    #[test]
    fn classify_navigation_blocked() {
        assert_eq!(
            classify("navigate", "HTTP 403 Forbidden"),
            FailureCategory::NavigationBlocked
        );
        assert_eq!(
            classify("navigate", "Access denied by server"),
            FailureCategory::NavigationBlocked
        );
    }

    #[test]
    fn classify_browser_crash() {
        assert_eq!(
            classify("click", "Bridge disconnected unexpectedly"),
            FailureCategory::BrowserCrash
        );
        assert_eq!(
            classify("navigate", "Target closed"),
            FailureCategory::BrowserCrash
        );
    }

    #[test]
    fn classify_javascript_error() {
        assert_eq!(
            classify("execute_js", "some random error"),
            FailureCategory::JavaScriptError
        );
        assert_eq!(
            classify("click", "Uncaught TypeError: cannot read property"),
            FailureCategory::JavaScriptError
        );
    }

    #[test]
    fn classify_captcha() {
        assert_eq!(
            classify("navigate", "CAPTCHA detected on page"),
            FailureCategory::CaptchaDetected
        );
        assert_eq!(
            classify("navigate", "Turnstile challenge triggered"),
            FailureCategory::CaptchaDetected
        );
    }

    #[test]
    fn classify_auth_required() {
        assert_eq!(
            classify("navigate", "Redirected to login page"),
            FailureCategory::AuthRequired
        );
        assert_eq!(
            classify("navigate", "401 Unauthorized"),
            FailureCategory::AuthRequired
        );
    }

    #[test]
    fn classify_rate_limited() {
        assert_eq!(
            classify("navigate", "429 Too Many Requests"),
            FailureCategory::RateLimited
        );
        assert_eq!(
            classify("navigate", "Rate limit exceeded, try again later"),
            FailureCategory::RateLimited
        );
    }

    #[test]
    fn classify_network_error() {
        assert_eq!(
            classify("navigate", "Network error: connection refused"),
            FailureCategory::NetworkError
        );
        assert_eq!(
            classify("navigate", "DNS resolution failed"),
            FailureCategory::NetworkError
        );
    }

    #[test]
    fn classify_form_validation() {
        assert_eq!(
            classify(
                "fill_form",
                "Form validation failed: required field missing"
            ),
            FailureCategory::FormValidation
        );
    }

    #[test]
    fn classify_budget_exceeded() {
        assert_eq!(
            classify("navigate", "Budget exceeded: $5.00 limit reached"),
            FailureCategory::BudgetExceeded
        );
        assert_eq!(
            classify("navigate", "Cost limit hit"),
            FailureCategory::BudgetExceeded
        );
    }

    #[test]
    fn classify_content_mismatch() {
        assert_eq!(
            classify("read_content", "Content not found under heading"),
            FailureCategory::ContentMismatch
        );
    }

    #[test]
    fn classify_unknown_default() {
        assert_eq!(
            classify("navigate", "something completely unexpected happened xyz"),
            FailureCategory::Unknown
        );
    }

    #[test]
    fn retry_strategy_selector_not_found_heals() {
        assert_eq!(
            retry_strategy(&FailureCategory::SelectorNotFound),
            RetryStrategy::RetryWithHealing
        );
        assert_eq!(
            retry_strategy(&FailureCategory::SelectorAmbiguous),
            RetryStrategy::RetryWithHealing
        );
    }

    #[test]
    fn retry_strategy_visibility_delays() {
        assert_eq!(
            retry_strategy(&FailureCategory::ElementNotVisible),
            RetryStrategy::RetryWithDelay(1)
        );
        assert_eq!(
            retry_strategy(&FailureCategory::ElementDisabled),
            RetryStrategy::RetryWithDelay(1)
        );
    }

    #[test]
    fn retry_strategy_timeout_delay() {
        assert_eq!(
            retry_strategy(&FailureCategory::NavigationTimeout),
            RetryStrategy::RetryWithDelay(2)
        );
    }

    #[test]
    fn retry_strategy_rate_limit_longer_delay() {
        assert_eq!(
            retry_strategy(&FailureCategory::RateLimited),
            RetryStrategy::RetryWithDelay(5)
        );
    }

    #[test]
    fn retry_strategy_browser_crash_resets() {
        assert_eq!(
            retry_strategy(&FailureCategory::BrowserCrash),
            RetryStrategy::ResetAndRetry
        );
    }

    #[test]
    fn retry_strategy_captcha_no_retry() {
        assert_eq!(
            retry_strategy(&FailureCategory::CaptchaDetected),
            RetryStrategy::NoRetry
        );
    }

    #[test]
    fn retry_strategy_budget_no_retry() {
        assert_eq!(
            retry_strategy(&FailureCategory::BudgetExceeded),
            RetryStrategy::NoRetry
        );
    }

    #[test]
    fn retry_strategy_unknown_no_retry() {
        assert_eq!(
            retry_strategy(&FailureCategory::Unknown),
            RetryStrategy::NoRetry
        );
    }
}
