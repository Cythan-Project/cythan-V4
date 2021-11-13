use std::borrow::Cow;

pub struct Template<'a> {
    pub pieces: Vec<TemplatePiece<'a>>,
    pub current_code_section: Cow<'a, str>,
}

impl Default for Template<'static> {
    fn default() -> Self {
        Template::new(include_str!("template.ct"))
    }
}

impl<'a> Template<'a> {
    pub fn new(string: &'a str) -> Self {
        let mut pieces = Vec::new();
        let mut current_template = Vec::new();
        for i in string.lines() {
            if let Some(e) = i.strip_prefix("# header ") {
                pieces.push(TemplatePiece::Section(current_template));
                current_template = vec![];
                pieces.push(TemplatePiece::NamedSection(Cow::Borrowed(e), vec![]));
            } else {
                current_template.push(Cow::Borrowed(i));
            }
        }
        pieces.push(TemplatePiece::Section(current_template));
        Self {
            pieces,
            current_code_section: Cow::Borrowed("CODE"),
        }
    }

    pub fn add_code(&mut self, string: Cow<'a, str>) {
        for i in self.pieces.iter_mut() {
            match i {
                TemplatePiece::Section(_) => (),
                TemplatePiece::NamedSection(a, b) => {
                    if a == &self.current_code_section {
                        b.push(string);
                        break;
                    }
                }
            }
        }
    }

    pub fn add_section(&mut self, section: &'a str, string: Cow<'a, str>) {
        for i in self.pieces.iter_mut() {
            match i {
                TemplatePiece::Section(_) => (),
                TemplatePiece::NamedSection(a, b) => {
                    if a == section {
                        b.push(string);
                        break;
                    }
                }
            }
        }
    }

    pub fn build(&self) -> String {
        self.pieces
            .iter()
            .map(|x| match x {
                TemplatePiece::Section(a) | TemplatePiece::NamedSection(_, a) => a.join("\n"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub trait Instruction {
    fn apply(&self, template: &mut Template);
}

pub enum TemplatePiece<'a> {
    Section(Vec<Cow<'a, str>>),
    NamedSection(Cow<'a, str>, Vec<Cow<'a, str>>),
}
