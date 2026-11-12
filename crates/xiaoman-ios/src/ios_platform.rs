//! iPhone app identifiers and guidance visible to the operator.

use std::collections::HashSet;

use xiaoman_api::AppInfo;

/// iPhone app identifiers and guidance visible to the operator.
pub struct IosPlatform;

impl IosPlatform {
    /// Default `WDA_BUNDLE_ID` of `scripts/ios/wda.sh`; Xcode installs the runner as `<id>.xctrunner`.
    pub const DEFAULT_WDA_BUNDLE_ID: &'static str = "com.xiaoman.WebDriverAgentRunner";
    /// Bundle id of the runner for the default [`IosPlatform::DEFAULT_WDA_BUNDLE_ID`].
    pub const WDA_BUNDLE_ID: &'static str = "com.xiaoman.WebDriverAgentRunner.xctrunner";

    /// Bundle id of the installed runner for a custom `WDA_BUNDLE_ID`.
    pub fn runner_bundle_id(wda_bundle_id: Option<&str>) -> String {
        let base = wda_bundle_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(Self::DEFAULT_WDA_BUNDLE_ID);
        format!("{base}.xctrunner")
    }

    /// Packages that are the home screen itself; launching them needs no approval.
    pub fn launcher_bundles() -> HashSet<String> {
        ["com.apple.springboard".to_string()].into_iter().collect()
    }

    /// Apps offered to the operator model, with the names iOS shows for them.
    pub fn default_apps() -> Vec<AppInfo> {
        [
            ("com.apple.Preferences", "设置"),
            ("com.apple.mobileslideshow", "照片"),
            ("com.apple.reminders", "提醒事项"),
            ("com.apple.mobilenotes", "备忘录"),
            ("com.apple.mobilesafari", "Safari 浏览器"),
            ("com.apple.mobiletimer", "时钟"),
            ("com.apple.mobilecal", "日历"),
            ("com.taobao.taobao4iphone", "淘宝"),
            ("com.tencent.xin", "微信"),
            ("com.alipay.iphoneclient", "支付宝"),
            ("com.autonavi.amap", "高德地图"),
        ]
        .into_iter()
        .map(|(package_name, label)| AppInfo {
            package_name: package_name.to_string(),
            label: label.to_string(),
        })
        .collect()
    }

    /// Platform notes appended to the operator system prompt.
    pub fn operator_notes() -> String {
        let apps = Self::default_apps()
            .iter()
            .map(|app| format!("{}={}", app.label, app.package_name))
            .collect::<Vec<_>>()
            .join("、");
        format!(
            "这是 iPhone。没有系统返回键，back 会点击左上角返回按钮或从左边缘向右滑动。\n\
             home 回到主屏幕。launch 的 packageName 填 iOS bundleId：\n\
             {apps}。\n\
             节点 bounds 和屏幕尺寸的单位是 iOS 点；动作 point 仍使用 0..1000 归一化坐标。\n\
             系统权限弹窗可能不在节点列表，需要看截图并用 point 点击。\n\
             不要尝试打开 WebDriverAgent（WebDriverAgentRunner）。"
        )
    }
}
