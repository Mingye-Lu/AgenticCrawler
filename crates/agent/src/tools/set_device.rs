use serde_json::Value;

/// Device preset for browser emulation.
#[derive(Debug, Clone)]
pub struct DevicePreset {
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub user_agent: &'static str,
    pub device_scale_factor: f64,
    pub is_mobile: bool,
    pub has_touch: bool,
}

impl DevicePreset {
    /// Convert to a JSON object suitable for the bridge `set_device` command.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "viewport": { "width": self.viewport_width, "height": self.viewport_height },
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
    user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    device_scale_factor: 3.0,
    is_mobile: true,
    has_touch: true,
};

const IPHONE_SE: DevicePreset = DevicePreset {
    viewport_width: 375,
    viewport_height: 667,
    user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    device_scale_factor: 2.0,
    is_mobile: true,
    has_touch: true,
};

const IPHONE_15_PRO_MAX: DevicePreset = DevicePreset {
    viewport_width: 430,
    viewport_height: 740,
    user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    device_scale_factor: 3.0,
    is_mobile: true,
    has_touch: true,
};

const PIXEL_7: DevicePreset = DevicePreset {
    viewport_width: 412,
    viewport_height: 915,
    user_agent: "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36",
    device_scale_factor: 2.625,
    is_mobile: true,
    has_touch: true,
};

const GALAXY_S24: DevicePreset = DevicePreset {
    viewport_width: 384,
    viewport_height: 832,
    user_agent: "Mozilla/5.0 (Linux; Android 14; Samsung Galaxy S24) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36",
    device_scale_factor: 2.625,
    is_mobile: true,
    has_touch: true,
};

const IPAD_PRO: DevicePreset = DevicePreset {
    viewport_width: 1024,
    viewport_height: 1366,
    user_agent: "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    device_scale_factor: 2.0,
    is_mobile: true,
    has_touch: true,
};

const IPAD: DevicePreset = DevicePreset {
    viewport_width: 768,
    viewport_height: 1024,
    user_agent: "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    device_scale_factor: 2.0,
    is_mobile: true,
    has_touch: true,
};

const GALAXY_TAB_S9: DevicePreset = DevicePreset {
    viewport_width: 800,
    viewport_height: 1280,
    user_agent: "Mozilla/5.0 (Linux; Android 13; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Safari/537.36",
    device_scale_factor: 2.0,
    is_mobile: true,
    has_touch: true,
};

const DESKTOP: DevicePreset = DevicePreset {
    viewport_width: 1920,
    viewport_height: 1080,
    user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    device_scale_factor: 1.0,
    is_mobile: false,
    has_touch: false,
};

const DESKTOP_HD: DevicePreset = DevicePreset {
    viewport_width: 1280,
    viewport_height: 720,
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
        assert_eq!(preset.viewport_height, 1080);
        assert!((preset.device_scale_factor - 1.0).abs() < f64::EPSILON);
        assert!(!preset.is_mobile);
        assert!(!preset.has_touch);
    }

    #[test]
    fn resolve_unknown_returns_none() {
        assert!(resolve_device("nonexistent").is_none());
        assert!(resolve_device("").is_none());
        assert!(resolve_device("iPhone 15").is_none());  // must be snake_case
    }

    #[test]
    fn resolve_all_presets_exist() {
        let names = [
            "iphone_15", "iphone_se", "iphone_15_pro_max", "pixel_7",
            "galaxy_s24", "ipad_pro", "ipad", "galaxy_tab_s9",
            "desktop", "desktop_hd",
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
}
