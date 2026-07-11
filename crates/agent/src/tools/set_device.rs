use std::collections::BTreeMap;

use serde_json::Value;

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

use super::feedback::InteractionKind;

/// Device preset for browser emulation.
#[derive(Debug, Clone)]
pub struct DevicePreset {
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub user_agent: &'static str,
    pub device_scale_factor: f64,
    pub is_mobile: bool,
    pub has_touch: bool,
}

impl DevicePreset {
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "viewport": { "width": self.viewport_width, "height": self.viewport_height },
            "screen": { "width": self.screen_width, "height": self.screen_height },
            "userAgent": self.user_agent,
            "deviceScaleFactor": self.device_scale_factor,
            "isMobile": self.is_mobile,
            "hasTouch": self.has_touch
        })
    }
}

// Preset data — Playwright canonical device descriptors.
const IPHONE_15: DevicePreset = DevicePreset {
    viewport_width: 393,
    viewport_height: 659,
    screen_width: 393,
    screen_height: 852,
    user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    device_scale_factor: 3.0,
    is_mobile: true,
    has_touch: true,
};

const IPHONE_SE: DevicePreset = DevicePreset {
    viewport_width: 375,
    viewport_height: 548,
    screen_width: 375,
    screen_height: 667,
    user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    device_scale_factor: 2.0,
    is_mobile: true,
    has_touch: true,
};

const IPHONE_15_PRO_MAX: DevicePreset = DevicePreset {
    viewport_width: 430,
    viewport_height: 740,
    screen_width: 430,
    screen_height: 932,
    user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    device_scale_factor: 3.0,
    is_mobile: true,
    has_touch: true,
};

const PIXEL_7: DevicePreset = DevicePreset {
    viewport_width: 412,
    viewport_height: 785,
    screen_width: 412,
    screen_height: 915,
    user_agent: "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
    device_scale_factor: 2.625,
    is_mobile: true,
    has_touch: true,
};

const GALAXY_S24: DevicePreset = DevicePreset {
    viewport_width: 360,
    viewport_height: 780,
    screen_width: 360,
    screen_height: 780,
    user_agent: "Mozilla/5.0 (Linux; Android 14; SM-S921B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
    device_scale_factor: 3.0,
    is_mobile: true,
    has_touch: true,
};

const IPAD_PRO: DevicePreset = DevicePreset {
    viewport_width: 1024,
    viewport_height: 1366,
    screen_width: 1024,
    screen_height: 1366,
    user_agent: "Mozilla/5.0 (iPad; CPU OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    device_scale_factor: 2.0,
    is_mobile: true,
    has_touch: true,
};

const IPAD: DevicePreset = DevicePreset {
    viewport_width: 768,
    viewport_height: 1024,
    screen_width: 768,
    screen_height: 1024,
    user_agent: "Mozilla/5.0 (iPad; CPU OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    device_scale_factor: 2.0,
    is_mobile: true,
    has_touch: true,
};

const GALAXY_TAB_S9: DevicePreset = DevicePreset {
    viewport_width: 800,
    viewport_height: 1280,
    screen_width: 800,
    screen_height: 1280,
    user_agent: "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    device_scale_factor: 2.0,
    is_mobile: true,
    has_touch: true,
};

const DESKTOP: DevicePreset = DevicePreset {
    viewport_width: 1920,
    viewport_height: 955,
    screen_width: 1920,
    screen_height: 1080,
    user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    device_scale_factor: 1.0,
    is_mobile: false,
    has_touch: false,
};

const DESKTOP_HD: DevicePreset = DevicePreset {
    viewport_width: 1366,
    viewport_height: 643,
    screen_width: 1366,
    screen_height: 768,
    user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    device_scale_factor: 1.0,
    is_mobile: false,
    has_touch: false,
};

/// Resolve a preset device name to its `DevicePreset`, or `None` if unknown.
#[must_use]
pub fn resolve_device(name: &str) -> Option<DevicePreset> {
    match name {
        "iphone_15" => Some(IPHONE_15.clone()),
        "iphone_se" => Some(IPHONE_SE.clone()),
        "iphone_15_pro_max" => Some(IPHONE_15_PRO_MAX.clone()),
        "pixel_7" => Some(PIXEL_7.clone()),
        "galaxy_s24" => Some(GALAXY_S24.clone()),
        "ipad_pro" => Some(IPAD_PRO.clone()),
        "ipad" => Some(IPAD.clone()),
        "galaxy_tab_s9" => Some(GALAXY_TAB_S9.clone()),
        "desktop" => Some(DESKTOP.clone()),
        "desktop_hd" => Some(DESKTOP_HD.clone()),
        _ => None,
    }
}

#[derive(Debug)]
enum DeviceInput {
    Preset(String),
    Custom {
        viewport: Option<(u32, u32)>,
        user_agent: Option<String>,
        device_scale_factor: Option<f64>,
        is_mobile: Option<bool>,
        has_touch: Option<bool>,
    },
}

fn parse_input(input: &Value) -> Result<DeviceInput, CrawlError> {
    if let Some(device) = input.get("device").and_then(Value::as_str) {
        let has_custom_fields = input.get("viewport").is_some()
            || input.get("userAgent").is_some()
            || input.get("deviceScaleFactor").is_some()
            || input.get("isMobile").is_some()
            || input.get("hasTouch").is_some();
        if has_custom_fields {
            return Err(CrawlError::new(
                "cannot mix 'device' preset with custom fields (viewport, userAgent, deviceScaleFactor, isMobile, hasTouch). Use either a preset name OR custom parameters.",
            ));
        }
        return Ok(DeviceInput::Preset(device.to_string()));
    }

    let viewport = if let Some(vp) = input.get("viewport") {
        let width = vp
            .get("width")
            .and_then(Value::as_u64)
            .ok_or_else(|| CrawlError::new("viewport.width must be a positive integer"))?;
        if width == 0 {
            return Err(CrawlError::new("viewport.width must be greater than zero"));
        }
        let height = vp
            .get("height")
            .and_then(Value::as_u64)
            .ok_or_else(|| CrawlError::new("viewport.height must be a positive integer"))?;
        if height == 0 {
            return Err(CrawlError::new("viewport.height must be greater than zero"));
        }
        Some((
            u32::try_from(width).map_err(|_| CrawlError::new("viewport.width out of range"))?,
            u32::try_from(height).map_err(|_| CrawlError::new("viewport.height out of range"))?,
        ))
    } else {
        None
    };

    let user_agent = input
        .get("userAgent")
        .and_then(Value::as_str)
        .map(str::to_string);
    let device_scale_factor = input.get("deviceScaleFactor").and_then(Value::as_f64);
    if let Some(device_scale_factor) = device_scale_factor {
        if device_scale_factor <= 0.0 {
            return Err(CrawlError::new(
                "deviceScaleFactor must be a positive number greater than zero",
            ));
        }
    }
    let is_mobile = input.get("isMobile").and_then(Value::as_bool);
    let has_touch = input.get("hasTouch").and_then(Value::as_bool);

    if viewport.is_none()
        && user_agent.is_none()
        && device_scale_factor.is_none()
        && is_mobile.is_none()
        && has_touch.is_none()
    {
        return Err(CrawlError::new(
            "must provide either 'device' (preset name) or at least one custom field (viewport, userAgent, deviceScaleFactor, isMobile, hasTouch)",
        ));
    }

    Ok(DeviceInput::Custom {
        viewport,
        user_agent,
        device_scale_factor,
        is_mobile,
        has_touch,
    })
}

fn custom_device_fingerprint(options: &Value) -> String {
    let sorted: BTreeMap<String, Value> = options
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    serde_json::to_string(&sorted).unwrap_or_else(|_| "custom".to_string())
}

/// Resolve a `DeviceInput` to a device name and JSON options for the bridge.
fn resolve_to_options(input: DeviceInput) -> Result<(String, Value), CrawlError> {
    match input {
        DeviceInput::Preset(name) => {
            let preset = resolve_device(&name).ok_or_else(|| {
                CrawlError::new(format!(
                    "unknown device preset '{name}'. Valid presets: iphone_15, iphone_se, iphone_15_pro_max, pixel_7, galaxy_s24, ipad_pro, ipad, galaxy_tab_s9, desktop, desktop_hd"
                ))
            })?;
            Ok((name, preset.to_json()))
        }
        DeviceInput::Custom {
            viewport,
            user_agent,
            device_scale_factor,
            is_mobile,
            has_touch,
        } => {
            let mut options = serde_json::json!({});
            if let Some((width, height)) = viewport {
                options["viewport"] = serde_json::json!({"width": width, "height": height});
            }
            if let Some(user_agent) = user_agent {
                options["userAgent"] = Value::String(user_agent);
            }
            if let Some(device_scale_factor) = device_scale_factor {
                options["deviceScaleFactor"] = serde_json::json!(device_scale_factor);
            }
            if let Some(is_mobile) = is_mobile {
                options["isMobile"] = serde_json::json!(is_mobile);
            }
            if let Some(has_touch) = has_touch {
                options["hasTouch"] = serde_json::json!(has_touch);
            }
            let fingerprint = custom_device_fingerprint(&options);
            Ok((format!("custom:{fingerprint}"), options))
        }
    }
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let device_input = parse_input(input)?;
    let widen = input
        .get("widen")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let (device_name, options) = resolve_to_options(device_input)?;
    let display_name = if device_name.starts_with("custom:") {
        "custom".to_string()
    } else {
        device_name.clone()
    };

    // Treat None (fresh browser default) as equivalent to "desktop" for no-op detection
    let is_noop = match crawl_state.current_device.as_deref() {
        Some(current) => current == device_name,
        None => device_name == "desktop",
    };

    if is_noop {
        return Ok(ToolEffect::reply_json(&serde_json::json!({
            "success": true,
            "message": format!("Already in '{}' mode — no change needed", display_name),
            "current_device": device_name,
        })));
    }

    if crawl_state.has_active_subagents {
        return Err(ToolExecutionError::new(
            "Cannot switch device while sub-agents are running. Wait for sub-agents to complete first.",
        ));
    }

    browser
        .acquire_bridge()
        .await
        .map_err(|error| ToolExecutionError::new(error.to_string()))?
        .set_device(&options)
        .await
        .map_err(|error| ToolExecutionError::new(error.to_string()))?;

    crawl_state.action_cache = None;
    crawl_state.current_device = Some(device_name.clone());

    let page_state = super::feedback::post_action_page_state(
        browser,
        crawl_state,
        InteractionKind::Passive,
        None,
        widen,
    )
    .await?;

    Ok(ToolEffect::reply_json(&serde_json::json!({
        "success": true,
        "message": format!("Switched to '{}' device mode", display_name),
        "current_device": device_name,
        "page_state": page_state,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_iphone_15_returns_correct_preset() {
        let preset = resolve_device("iphone_15").unwrap();
        assert_eq!(preset.viewport_width, 393);
        assert_eq!(preset.viewport_height, 659);
        assert!((preset.device_scale_factor - 3.0).abs() < f64::EPSILON);
        assert!(preset.is_mobile);
        assert!(preset.has_touch);
    }

    #[test]
    fn resolve_desktop_returns_desktop_defaults() {
        let preset = resolve_device("desktop").unwrap();
        assert_eq!(preset.viewport_width, 1920);
        assert_eq!(preset.viewport_height, 955);
        assert!((preset.device_scale_factor - 1.0).abs() < f64::EPSILON);
        assert!(!preset.is_mobile);
        assert!(!preset.has_touch);
    }

    #[test]
    fn resolve_unknown_returns_none() {
        assert!(resolve_device("nonexistent").is_none());
        assert!(resolve_device("").is_none());
        assert!(resolve_device("iPhone 15").is_none()); // must be snake_case
    }

    #[test]
    fn resolve_all_presets_exist() {
        let names = [
            "iphone_15",
            "iphone_se",
            "iphone_15_pro_max",
            "pixel_7",
            "galaxy_s24",
            "ipad_pro",
            "ipad",
            "galaxy_tab_s9",
            "desktop",
            "desktop_hd",
        ];
        for name in &names {
            assert!(resolve_device(name).is_some(), "preset missing: {name}");
        }
    }

    #[test]
    fn to_json_produces_correct_shape() {
        let preset = resolve_device("pixel_7").unwrap();
        let json = preset.to_json();
        assert!(json["viewport"]["width"].as_u64().is_some());
        assert!(json["viewport"]["height"].as_u64().is_some());
        assert!(json["userAgent"].as_str().is_some());
        assert!(json["deviceScaleFactor"].as_f64().is_some());
        assert!(json["isMobile"].as_bool().is_some());
        assert!(json["hasTouch"].as_bool().is_some());
    }

    #[test]
    fn parse_device_preset_input() {
        let input = serde_json::json!({"device": "iphone_15"});
        let result = parse_input(&input).unwrap();
        assert!(matches!(result, DeviceInput::Preset(name) if name == "iphone_15"));
    }

    #[test]
    fn parse_custom_input() {
        let input = serde_json::json!({
            "viewport": {"width": 375, "height": 812},
            "userAgent": "Test UA",
            "deviceScaleFactor": 2.0,
            "isMobile": true,
            "hasTouch": true
        });
        let result = parse_input(&input).unwrap();
        assert!(matches!(result, DeviceInput::Custom { .. }));
    }

    #[test]
    fn parse_empty_input_returns_error() {
        let input = serde_json::json!({});
        let result = parse_input(&input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must provide either"));
    }

    #[test]
    fn resolve_unknown_preset_returns_error() {
        let input = serde_json::json!({"device": "nonexistent_device"});
        let parsed = parse_input(&input).unwrap();
        let result = resolve_to_options(parsed);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unknown device preset"));
    }

    #[test]
    fn resolve_iphone_15_preset_to_options() {
        let input = serde_json::json!({"device": "iphone_15"});
        let parsed = parse_input(&input).unwrap();
        let (name, options) = resolve_to_options(parsed).unwrap();
        assert_eq!(name, "iphone_15");
        assert_eq!(options["viewport"]["width"], 393);
        assert_eq!(options["isMobile"], true);
    }

    #[test]
    fn mixed_preset_and_custom_returns_error() {
        let input =
            serde_json::json!({"device": "iphone_15", "viewport": {"width": 400, "height": 800}});
        let result = parse_input(&input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot mix"));
    }
}
