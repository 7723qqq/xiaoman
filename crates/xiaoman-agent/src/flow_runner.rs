//! Chinese/English yes-no judgement for flow confirmations.

const YES_ZH: [&str; 8] = ["是", "好", "确认", "确定", "可以", "行", "对", "嗯"];
const YES_EN: [&str; 6] = ["yes", "ok", "okay", "y", "sure", "yeah"];

/// 是/好/确认/yes/ok and close variants (好的, 确认呼叫, OK!) count as yes; 不…/no/别… do not.
pub fn is_yes(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    if answer.is_empty()
        || answer.starts_with('不')
        || answer.starts_with('别')
        || answer.starts_with("no")
    {
        return false;
    }
    if YES_ZH.iter().any(|prefix| answer.starts_with(prefix)) {
        return true;
    }
    YES_EN.iter().any(|prefix| {
        if !answer.starts_with(prefix) {
            return false;
        }
        answer[prefix.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_alphabetic())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yes_words() {
        for text in ["是", "好", "好的", "确认", "yes", "OK", "ok!", "Yes please"] {
            assert!(is_yes(text), "{text}");
        }
        for text in ["不", "不要", "不好", "no", "nope", "取消", "", "okra"] {
            assert!(!is_yes(text), "{text}");
        }
    }
}
