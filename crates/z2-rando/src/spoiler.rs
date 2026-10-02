//! Plain-text spoiler log.
//!
//! Modules append lines to named sections ([`Spoiler::section`]). The log
//! prints the header, a contents line, the [`OPTIONS_SECTION`] (the
//! options that differ from vanilla), and then every other section in the
//! order it was first touched, which is the fixed pipeline order, so the
//! log is deterministic. The frontends write it with `--rando-spoiler PATH`.

/// Section listing the changed options; printed first.
pub const OPTIONS_SECTION: &str = "Options";

/// The spoiler being built for one seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spoiler {
    header: Vec<(String, String)>,
    sections: Vec<(String, Vec<String>)>,
}

impl Spoiler {
    /// Empty spoiler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a header line (`key: value`); replaces an existing key.
    pub fn set_header(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.header.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.header.push((key.to_string(), value)),
        }
    }

    /// Lines of section `title`, created on first use.
    pub fn section(&mut self, title: &str) -> &mut Vec<String> {
        let i = match self.sections.iter().position(|(t, _)| t == title) {
            Some(i) => i,
            None => {
                self.sections.push((title.to_string(), Vec::new()));
                self.sections.len() - 1
            }
        };
        &mut self.sections[i].1
    }

    /// Append one line to section `title`.
    pub fn line(&mut self, title: &str, text: impl Into<String>) {
        self.section(title).push(text.into());
    }

    /// Whether nothing beyond the header was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sections.iter().all(|(_, l)| l.is_empty())
    }

    /// Titles of the sections that have lines, in print order.
    #[must_use]
    pub fn titles(&self) -> Vec<&str> {
        self.ordered().map(|(t, _)| t.as_str()).collect()
    }

    /// Lines of section `title`, if it exists.
    #[must_use]
    pub fn lines(&self, title: &str) -> Option<&[String]> {
        self.sections
            .iter()
            .find(|(t, _)| t == title)
            .map(|(_, l)| l.as_slice())
    }

    fn ordered(&self) -> impl Iterator<Item = &(String, Vec<String>)> {
        let options = self
            .sections
            .iter()
            .filter(|(t, l)| t == OPTIONS_SECTION && !l.is_empty());
        let rest = self
            .sections
            .iter()
            .filter(|(t, l)| t != OPTIONS_SECTION && !l.is_empty());
        options.chain(rest)
    }

    /// Render the whole log.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from("z2rs randomizer spoiler\n");
        for (k, v) in &self.header {
            out.push_str(&format!("{k}: {v}\n"));
        }
        let titles = self.titles();
        if !titles.is_empty() {
            out.push_str(&format!("Contents: {}\n", titles.join(", ")));
        }
        for (title, lines) in self.ordered() {
            out.push('\n');
            out.push_str(&format!("== {title} ==\n"));
            for l in lines {
                out.push_str(l);
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_header_and_sections_in_first_use_order() {
        let mut s = Spoiler::new();
        s.set_header("Seed", "abc");
        s.set_header("Seed", "xyz");
        s.line("Items", "Glove: Palace 2");
        s.line("Palaces", "P1: Tower");
        s.line("Items", "Raft: Palace 3");
        s.section("Empty");
        let r = s.render();
        assert!(r.contains("Seed: xyz\n"));
        assert!(!r.contains("abc"));
        let items = r.find("== Items ==").unwrap();
        let palaces = r.find("== Palaces ==").unwrap();
        assert!(items < palaces);
        assert!(r.contains("Glove: Palace 2\nRaft: Palace 3\n"));
        assert!(!r.contains("Empty"));
        assert!(!s.is_empty());
        assert!(Spoiler::new().is_empty());
        assert!(r.contains("Contents: Items, Palaces\n"));

        // The options section always prints first.
        s.line(OPTIONS_SECTION, "hints.helpful_hints = By continent");
        let r = s.render();
        assert!(r.find("== Options ==").unwrap() < r.find("== Items ==").unwrap());
        assert_eq!(s.titles(), vec!["Options", "Items", "Palaces"]);
        assert_eq!(s.lines("Palaces").unwrap(), ["P1: Tower".to_string()]);
    }
}
