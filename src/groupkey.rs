//! Error texts grouped by their wording, so a check can say "557 models: B. part is N bytes ...".

/// The key under which an error message is counted: every run of digits becomes `N`.
pub trait GroupKey {
    fn group_key(&self) -> String;
}

impl GroupKey for str {
    fn group_key(&self) -> String {
        let mut out = String::with_capacity(self.len());
        let mut in_number = false;
        for c in self.chars() {
            if c.is_ascii_digit() {
                if !in_number {
                    out.push('N');
                }
                in_number = true;
            } else {
                in_number = false;
                out.push(c);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_become_n() {
        assert_eq!(
            "B. part is 123456 bytes, expected 99".group_key(),
            "B. part is N bytes, expected N"
        );
        assert_eq!("no digits".group_key(), "no digits");
        assert_eq!("a1b22c".group_key(), "aNbNc");
    }
}
