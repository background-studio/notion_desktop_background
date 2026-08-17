use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FitMode {
    #[default]
    Cover,
    Contain,
    Fill,
    Tile,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySettings {
    pub fit: FitMode,
    pub position_x: f64,
    pub position_y: f64,
    pub opacity: f64,
    pub blur: f64,
    pub scale: f64,
    pub overlay_color: String,
    pub overlay_opacity: f64,
    pub block_fill_opacity: f64,
    pub home_intensity: f64,
    pub task_intensity: f64,
    pub sidebar_opacity: f64,
    pub surface_opacity: f64,
    pub composer_opacity: f64,
    pub menu_opacity: f64,
    pub terminal_opacity: f64,
    pub enabled_on_home: bool,
    pub enabled_on_tasks: bool,
    pub video_muted: bool,
    pub video_playback_rate: f64,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            fit: FitMode::Cover,
            position_x: 50.0,
            position_y: 50.0,
            opacity: 0.72,
            blur: 0.0,
            scale: 1.0,
            overlay_color: "#101416".to_string(),
            overlay_opacity: 0.12,
            block_fill_opacity: 0.55,
            home_intensity: 1.0,
            task_intensity: 0.32,
            sidebar_opacity: 0.18,
            surface_opacity: 0.12,
            composer_opacity: 0.88,
            menu_opacity: 0.9,
            terminal_opacity: 0.9,
            enabled_on_home: true,
            enabled_on_tasks: true,
            video_muted: true,
            video_playback_rate: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub phase: String,
    pub message: String,
    pub active_targets: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notion_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            phase: "idle".to_string(),
            message: "尚未配置背景".to_string(),
            active_targets: 0,
            notion_version: None,
            last_error: None,
        }
    }
}

fn required_object<'a>(
    value: &'a Value,
    key: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    value
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("缺少必需字段：{key}"))
}

fn required_number(map: &serde_json::Map<String, Value>, key: &str) -> Result<f64, String> {
    let value = map.get(key).ok_or_else(|| format!("缺少必需字段：{key}"))?;
    let number = value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))
        .ok_or_else(|| format!("字段 {key} 必须是数字。"))?;
    if !number.is_finite() {
        return Err(format!("字段 {key} 不是有效数字。"));
    }
    Ok(number)
}

fn required_bool(map: &serde_json::Map<String, Value>, key: &str) -> Result<bool, String> {
    map.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("缺少必需字段：{key}"))
}

fn required_string<'a>(
    map: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a str, String> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("缺少必需字段：{key}"))
}

fn clamp_required(value: f64, key: &str, minimum: f64, maximum: f64) -> Result<f64, String> {
    if value < minimum || value > maximum {
        return Err(format!("字段 {key} 超出范围 {minimum}..={maximum}。"));
    }
    Ok(value)
}

impl DisplaySettings {
    pub fn from_configure(value: &Value) -> Result<Self, String> {
        let display = required_object(value, "display")?;
        let fit = match required_string(display, "fit")? {
            "cover" => FitMode::Cover,
            "contain" => FitMode::Contain,
            "fill" => FitMode::Fill,
            "tile" => FitMode::Tile,
            other => return Err(format!("不支持的 fit：{other}")),
        };
        let overlay_color = required_string(display, "overlayColor")?;
        if overlay_color.len() != 7
            || !overlay_color.starts_with('#')
            || !overlay_color[1..]
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err("overlayColor 必须是 #RRGGBB。".to_string());
        }
        Ok(Self {
            fit,
            position_x: clamp_required(
                required_number(display, "positionX")?,
                "positionX",
                0.0,
                100.0,
            )?,
            position_y: clamp_required(
                required_number(display, "positionY")?,
                "positionY",
                0.0,
                100.0,
            )?,
            opacity: clamp_required(required_number(display, "opacity")?, "opacity", 0.0, 1.0)?,
            blur: clamp_required(required_number(display, "blur")?, "blur", 0.0, 40.0)?,
            scale: clamp_required(required_number(display, "scale")?, "scale", 1.0, 1.3)?,
            overlay_color: overlay_color.to_ascii_lowercase(),
            overlay_opacity: clamp_required(
                required_number(display, "overlayOpacity")?,
                "overlayOpacity",
                0.0,
                0.9,
            )?,
            block_fill_opacity: clamp_required(
                required_number(display, "blockFillOpacity")?,
                "blockFillOpacity",
                0.0,
                1.0,
            )?,
            home_intensity: clamp_required(
                required_number(display, "homeIntensity")?,
                "homeIntensity",
                0.0,
                1.0,
            )?,
            task_intensity: clamp_required(
                required_number(display, "taskIntensity")?,
                "taskIntensity",
                0.0,
                1.0,
            )?,
            sidebar_opacity: clamp_required(
                required_number(display, "sidebarOpacity")?,
                "sidebarOpacity",
                0.0,
                1.0,
            )?,
            surface_opacity: clamp_required(
                required_number(display, "surfaceOpacity")?,
                "surfaceOpacity",
                0.0,
                1.0,
            )?,
            composer_opacity: clamp_required(
                required_number(display, "composerOpacity")?,
                "composerOpacity",
                0.0,
                1.0,
            )?,
            menu_opacity: clamp_required(
                required_number(display, "menuOpacity")?,
                "menuOpacity",
                0.0,
                1.0,
            )?,
            terminal_opacity: clamp_required(
                required_number(display, "terminalOpacity")?,
                "terminalOpacity",
                0.0,
                1.0,
            )?,
            enabled_on_home: required_bool(display, "enabledOnHome")?,
            enabled_on_tasks: required_bool(display, "enabledOnTasks")?,
            video_muted: required_bool(display, "videoMuted")?,
            video_playback_rate: clamp_required(
                required_number(display, "videoPlaybackRate")?,
                "videoPlaybackRate",
                0.25,
                2.0,
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_missing_display_fields() {
        let error = DisplaySettings::from_configure(&json!({
            "display": { "fit": "cover" }
        }))
        .unwrap_err();
        assert!(error.contains("缺少必需字段"));
    }

    #[test]
    fn rejects_out_of_range_display_fields() {
        let mut value = serde_json::to_value(DisplaySettings::default()).unwrap();
        value["opacity"] = json!(2.0);
        let error = DisplaySettings::from_configure(&json!({ "display": value })).unwrap_err();
        assert!(error.contains("opacity"));
    }
}
