pub(crate) fn calculate(input: &str) -> Option<String> {
    if input.len() > 256
        || !input
            .chars()
            .any(|c| matches!(c, '+' | '-' | '*' | '/' | '('))
    {
        return None;
    }
    struct Parser<'a> {
        bytes: &'a [u8],
        position: usize,
    }
    impl Parser<'_> {
        fn space(&mut self) {
            while self
                .bytes
                .get(self.position)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.position += 1;
            }
        }
        fn expression(&mut self, minimum: u8) -> Option<f64> {
            self.space();
            let mut left = match self.bytes.get(self.position).copied()? {
                b'+' | b'-' => {
                    let negative = self.bytes[self.position] == b'-';
                    self.position += 1;
                    let number = self.expression(3)?;
                    if negative {
                        -number
                    } else {
                        number
                    }
                }
                b'(' => {
                    self.position += 1;
                    let number = self.expression(0)?;
                    self.space();
                    if self.bytes.get(self.position) != Some(&b')') {
                        return None;
                    }
                    self.position += 1;
                    number
                }
                _ => {
                    let start = self.position;
                    while self
                        .bytes
                        .get(self.position)
                        .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
                    {
                        self.position += 1;
                    }
                    std::str::from_utf8(&self.bytes[start..self.position])
                        .ok()?
                        .parse::<f64>()
                        .ok()?
                }
            };
            loop {
                self.space();
                let Some(operator) = self.bytes.get(self.position).copied() else {
                    break;
                };
                let priority = match operator {
                    b'+' | b'-' => 1,
                    b'*' | b'/' => 2,
                    _ => break,
                };
                if priority < minimum {
                    break;
                }
                self.position += 1;
                let right = self.expression(priority + 1)?;
                left = match operator {
                    b'+' => left + right,
                    b'-' => left - right,
                    b'*' => left * right,
                    b'/' if right != 0.0 => left / right,
                    _ => return None,
                };
                if !left.is_finite() {
                    return None;
                }
            }
            Some(left)
        }
    }
    let mut parser = Parser {
        bytes: input.as_bytes(),
        position: 0,
    };
    let result = parser.expression(0)?;
    parser.space();
    if parser.position != input.len() {
        return None;
    }
    Some(if result == 0.0 {
        "0".into()
    } else {
        result.to_string()
    })
}
