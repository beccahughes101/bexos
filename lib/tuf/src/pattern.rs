//! Case-sensitive shell patterns for TUF delegation paths, with bounded work.
use alloc::vec::Vec;

pub(super) fn matches(pattern: &str, path: &str) -> bool {
    if pattern.len() > 1024 || path.len() > 1024 {
        return false;
    }
    let pattern: Vec<_> = pattern.chars().collect();
    let path: Vec<_> = path.chars().collect();
    let mut matched = alloc::vec![false; path.len() + 1];
    matched[0] = true;
    let mut offset = 0;
    while offset < pattern.len() {
        let token = pattern[offset];
        offset += 1;
        if token == '*' {
            for i in 1..matched.len() {
                matched[i] |= matched[i - 1];
            }
            continue;
        }
        let mut class = None;
        if token == '[' {
            let mut end = offset;
            if pattern.get(end) == Some(&'!') {
                end += 1;
            }
            // A closing bracket at the start of a class is a literal member.
            if pattern.get(end) == Some(&']') {
                end += 1;
            }
            while end < pattern.len() && pattern[end] != ']' {
                end += 1;
            }
            if end < pattern.len() {
                class = Some(&pattern[offset..end]);
                offset = end + 1;
            }
        }
        for i in (1..matched.len()).rev() {
            let accepts = match class {
                Some(class) => in_class(class, path[i - 1]),
                None => token == '?' || token == path[i - 1],
            };
            matched[i] = matched[i - 1] && accepts;
        }
        matched[0] = false;
    }
    matched[path.len()]
}

fn in_class(mut class: &[char], value: char) -> bool {
    let negated = class.first() == Some(&'!');
    if negated {
        class = &class[1..];
    }
    let mut found = false;
    while !class.is_empty() {
        if class.len() >= 3 && class[1] == '-' {
            found |= class[0] <= value && value <= class[2];
            class = &class[3..];
        } else {
            found |= class[0] == value;
            class = &class[1..];
        }
    }
    found != negated
}
