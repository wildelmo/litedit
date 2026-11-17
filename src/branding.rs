//! Who made this build, for Help → About: a name, a company and an email
//! address from `assets/branding.txt`, embedded when the program is built.
//! Nothing is read at run time and nothing is sent anywhere.

/// The longest value shown (anything longer is cut).
const MAX_LEN: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branding {
    pub name: String,
    pub company: String,
    pub email: String,
}

/// The identity built into this exe. Without a name in the file, the
/// author from `Cargo.toml` is used.
pub fn embedded() -> Branding {
    let mut b = parse(include_str!("../assets/branding.txt"));
    if b.name.is_empty() {
        b.name = clean(env!("CARGO_PKG_AUTHORS"));
    }
    b
}

/// `key = value` lines; `#` starts a comment line. Unknown keys and lines
/// without `=` are skipped, a repeated key keeps its last value.
pub fn parse(text: &str) -> Branding {
    let mut b = Branding::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = clean(value);
        match key.trim() {
            "name" => b.name = value,
            "company" => b.company = value,
            "email" => b.email = value,
            _ => {}
        }
    }
    b
}

/// One line of plain text: no control characters, not too long.
fn clean(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LEN)
        .collect::<String>()
        .trim()
        .to_string()
}

impl Branding {
    /// The rows About shows, in order: (label, value). Empty values are
    /// left out.
    pub fn rows(&self) -> Vec<(&'static str, &str)> {
        [
            ("Created by", self.name.as_str()),
            ("Company", self.company.as_str()),
            ("Email", self.email.as_str()),
        ]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_fields_and_skips_the_rest() {
        let b = parse(
            "# comment = not a field\r\n\r\nname = Ada Lovelace \r\ncompany=Analytical Engines = Ltd\n\
             email = ada@example.com\nphone = 123\nno equals sign\nname = Ada L.\n",
        );
        assert_eq!(b.name, "Ada L.", "the last value wins");
        assert_eq!(b.company, "Analytical Engines = Ltd");
        assert_eq!(b.email, "ada@example.com");
    }

    #[test]
    fn empty_rows_are_not_shown() {
        let b = parse("name = Ada\ncompany =\nemail =   \n");
        assert_eq!(b.rows(), vec![("Created by", "Ada")]);
        let all = parse("name = Ada\ncompany = Engines\nemail = a@example.com\n");
        assert_eq!(
            all.rows(),
            vec![
                ("Created by", "Ada"),
                ("Company", "Engines"),
                ("Email", "a@example.com")
            ]
        );
        assert!(parse("").rows().is_empty());
    }

    #[test]
    fn values_are_one_short_line_of_plain_text() {
        let long = format!(
            "name = {}\ncompany = a\tb\u{1b}[31mc\u{0}\n",
            "x".repeat(500)
        );
        let b = parse(&long);
        assert_eq!(b.name.chars().count(), MAX_LEN);
        assert_eq!(b.company, "ab[31mc");
        // Any bytes at all: never a panic.
        let junk =
            String::from_utf8_lossy(&[0xff, b'=', 0x80, b'\n', b'=', b'=', 0xf0]).into_owned();
        let _ = parse(&junk).rows();
    }

    #[test]
    fn the_embedded_file_names_who_built_it() {
        let b = embedded();
        assert!(!b.name.is_empty());
        assert_eq!(b.rows()[0].0, "Created by");
    }
}
