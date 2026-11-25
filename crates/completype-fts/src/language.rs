use std::fmt;

use frostem::Algorithm;

macro_rules! languages {
    ($($variant:ident $name:literal $(stop $file:literal)?,)*) => {
        /// A language with a Snowball stemmer.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Language {
            $($variant,)*
        }

        impl Language {
            pub const ALL: &'static [Language] = &[$(Language::$variant,)*];

            /// The Snowball name, such as `english`.
            pub fn name(self) -> &'static str {
                match self {
                    $(Language::$variant => $name,)*
                }
            }

            pub(crate) fn algorithm(self) -> Algorithm {
                match self {
                    $(Language::$variant => Algorithm::$variant,)*
                }
            }

            fn snowball_list(self) -> Option<&'static str> {
                match self {
                    $($(Language::$variant => Some(include_str!(concat!("../stopwords/", $file))),)?)*
                    #[allow(unreachable_patterns)]
                    _ => None,
                }
            }
        }
    };
}

languages! {
    Arabic "arabic",
    Armenian "armenian",
    Basque "basque",
    Catalan "catalan",
    Czech "czech",
    Danish "danish" stop "danish.txt",
    Dutch "dutch" stop "dutch.txt",
    English "english" stop "english.txt",
    Esperanto "esperanto",
    Estonian "estonian",
    Finnish "finnish" stop "finnish.txt",
    French "french" stop "french.txt",
    German "german" stop "german.txt",
    Greek "greek",
    Hindi "hindi",
    Hungarian "hungarian" stop "hungarian.txt",
    Indonesian "indonesian" stop "indonesian.txt",
    Irish "irish" stop "irish.txt",
    Italian "italian" stop "italian.txt",
    Lithuanian "lithuanian",
    Nepali "nepali",
    Norwegian "norwegian" stop "norwegian.txt",
    Persian "persian",
    Polish "polish",
    Portuguese "portuguese" stop "portuguese.txt",
    Romanian "romanian",
    Russian "russian" stop "russian.txt",
    Serbian "serbian",
    Sesotho "sesotho" stop "sesotho.txt",
    Spanish "spanish" stop "spanish.txt",
    Swedish "swedish" stop "swedish.txt",
    Tamil "tamil",
    Turkish "turkish",
    Ukrainian "ukrainian" stop "ukrainian.txt",
    Yiddish "yiddish",
}

impl Language {
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|l| l.name() == name)
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Words left out of queries, never out of the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StopWords {
    words: Vec<String>,
}

impl StopWords {
    pub fn new<W: Into<String>>(words: impl IntoIterator<Item = W>) -> Self {
        Self::default().with(words)
    }

    /// The Snowball project's list for `language`, if it has one.
    pub fn snowball(language: Language) -> Option<Self> {
        let list = language.snowball_list()?;
        Some(Self::new(list.lines().flat_map(|line| {
            line.split('|').next().unwrap_or("").split_whitespace()
        })))
    }

    /// Adds `words` to the list.
    pub fn with<W: Into<String>>(mut self, words: impl IntoIterator<Item = W>) -> Self {
        self.words.extend(words.into_iter().map(Into::into));
        self
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }
}
