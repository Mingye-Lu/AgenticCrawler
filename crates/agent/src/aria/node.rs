/// Confirmed state flags from the T1 spec.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AriaStates {
    pub disabled: bool,
    pub checked: bool,
    pub expanded: Option<bool>,
    pub pressed: Option<bool>,
    pub selected: bool,
    pub level: Option<u8>,
    pub active: bool,
    pub invalid: bool,
}

/// A single node in the ARIA tree.
#[derive(Debug, Clone, PartialEq)]
pub struct AriaNode {
    pub role: String,
    pub name: Option<String>,
    pub states: AriaStates,
    pub ref_id: Option<String>,
    pub url: Option<String>,
    pub frame_id: Option<String>,
    pub offscreen: bool,
    pub children: Vec<AriaNode>,
    pub omitted_children: usize,
}
