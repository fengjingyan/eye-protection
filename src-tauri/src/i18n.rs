// 国际化：locale JSON 直接在编译期嵌入二进制，运行时无需外部文件。

use serde_json::Value;

const ZH_CN: &str = include_str!("../../ui/i18n/zh-CN.json");
const EN: &str = include_str!("../../ui/i18n/en.json");

pub fn load_locale(lang: &str) -> Value {
    let src = if lang == "en" || lang.starts_with("en") {
        EN
    } else {
        ZH_CN
    };
    serde_json::from_str(src).unwrap_or(Value::Null)
}

// 按点分 key 取翻译；缺失时回退为 key 本身
pub fn l(v: &Value, key: &str) -> String {
    let mut cur = v;
    for part in key.split('.') {
        match cur.get(part) {
            Some(next) => cur = next,
            None => return key.to_string(),
        }
    }
    cur.as_str().map(|s| s.to_string()).unwrap_or_else(|| key.to_string())
}
