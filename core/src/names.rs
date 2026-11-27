//! How a Profile is called (GLOSSARY: Profile name, Short label): the one
//! place those names are made.
//!
//! - A **Profile name** is the person's full name and the Login's email,
//!   plus the Organization when it isn't the personal default or two
//!   Profiles would otherwise share a name.
//! - A **Short label** is the nickname, else the person's first name,
//!   lengthened only as far as needed to tell two Profiles apart.
//!
//! A nickname replaces either; the caller applies it to the Profile name.

/// The names Claude gives a Login and its Organization (`oauthAccount`), and
/// the names the app makes from them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LoginNames {
    /// Empty when missing.
    pub email: String,
    /// Empty when missing.
    pub organization_name: String,
    /// `fullName`; `None` when missing or empty.
    pub full_name: Option<String>,
    /// `displayName`; `None` when missing or empty.
    pub display_name: Option<String>,
}

impl LoginNames {
    /// The Organization when it says more than the email: not empty and not
    /// Claude's personal default, `<email>'s Organization`.
    fn telling_organization(&self) -> Option<&str> {
        let name = self.organization_name.as_str();
        (!name.is_empty() && name != self.personal_default()).then_some(name)
    }

    /// What Claude calls this Login's personal default Organization.
    fn personal_default(&self) -> String {
        format!("{}'s Organization", self.email)
    }

    /// The Profile name: `<fullName> · <email>` (else the display name, else
    /// the email alone), then ` · <Organization>` unless it is the personal
    /// default.
    pub fn name(&self) -> String {
        self.joined(self.telling_organization())
    }

    /// The Profile name with the Organization even when it is the personal
    /// default, for two Profiles that would otherwise share a name.
    pub fn name_with_organization(&self) -> String {
        self.joined(Some(self.organization_name.as_str()))
    }

    fn joined(&self, organization: Option<&str>) -> String {
        let person = self.full_name.as_deref().or(self.display_name.as_deref());
        [person, Some(self.email.as_str()), organization]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The Organization is Claude's personal default (`<email>'s
    /// Organization`), which only its owner can be in.
    pub fn personal_organization(&self) -> bool {
        !self.email.is_empty() && self.organization_name == self.personal_default()
    }

    /// The Organization as people call it: its name when that tells, else
    /// the person's (`Ada Lovelace's Organization`) for the personal default.
    pub fn organization_label(&self) -> String {
        let person = self.full_name.as_deref().or(self.display_name.as_deref());
        match (self.telling_organization(), person) {
            (Some(organization), _) => organization.to_owned(),
            (None, Some(person)) => format!("{person}'s Organization"),
            (None, None) => self.organization_name.clone(),
        }
    }

    /// `email · Organization`, for the hover line.
    pub fn email_and_organization(&self) -> String {
        format!("{} · {}", self.email, self.organization_name)
    }

    /// The short labels, shortest first: the display name (else the full
    /// name's first word, else the email's local part); it with the
    /// Organization when that tells; it with the email (the email alone in
    /// place of the local part); and that with the Organization.
    pub fn short_labels(&self) -> Vec<String> {
        let first_name = self
            .full_name
            .as_deref()
            .and_then(|name| name.split_whitespace().next());
        let person = self.display_name.as_deref().or(first_name);
        let short = person
            .unwrap_or_else(|| self.email.split('@').next().unwrap_or_default())
            .to_owned();
        let with_email = match person {
            Some(person) => format!("{person} · {}", self.email),
            None => self.email.clone(),
        };
        let mut labels = vec![short.clone()];
        if let Some(organization) = self.telling_organization() {
            labels.push(format!("{short} · {organization}"));
        }
        if !self.organization_name.is_empty() {
            labels.push(with_email.clone());
            labels.push(format!("{with_email} · {}", self.organization_name));
        } else {
            labels.push(with_email);
        }
        labels
    }
}

/// Each Profile's Profile name, in order, from its Login (`None` for a
/// signed-out Profile): [`LoginNames::name`], with the Organization added
/// when two Profiles would otherwise share it.
pub(crate) fn profile_names<'a>(
    logins: impl IntoIterator<Item = Option<&'a LoginNames>>,
) -> Vec<String> {
    unique(
        logins
            .into_iter()
            .map(|login| match login {
                Some(names) => vec![names.name(), names.name_with_organization()],
                None => vec!["Signed-out Profile".to_owned()],
            })
            .collect(),
    )
}

/// Each Profile's Short label, in order, from its nickname and its Login
/// (`None` for a signed-out Profile): the nickname, else the shortest of
/// [`LoginNames::short_labels`] no other Profile shares (two Profiles named
/// `Lucie` become `Lucie · Work Inc` and `Lucie · Research Lab`).
pub(crate) fn short_labels<'a>(
    profiles: impl IntoIterator<Item = (Option<&'a str>, Option<&'a LoginNames>)>,
) -> Vec<String> {
    unique(
        profiles
            .into_iter()
            .map(|(nickname, login)| match (nickname, login) {
                (Some(nickname), _) => vec![nickname.to_owned()],
                (None, Some(names)) => names.short_labels(),
                (None, None) => vec!["Signed out".to_owned()],
            })
            .collect(),
    )
}

/// One pick per list of choices (shortest first): the first no other list's
/// pick shares. Colliding lists move on together until none collides or a
/// list runs out of choices.
fn unique(choices: Vec<Vec<String>>) -> Vec<String> {
    let mut level = vec![0; choices.len()];
    loop {
        let label = |index: usize| &choices[index][level[index]];
        let collided: Vec<usize> = (0..choices.len())
            .filter(|&index| {
                level[index] + 1 < choices[index].len()
                    && (0..choices.len())
                        .any(|other| other != index && label(other) == label(index))
            })
            .collect();
        if collided.is_empty() {
            break;
        }
        for index in collided {
            level[index] += 1;
        }
    }
    choices
        .into_iter()
        .zip(level)
        .map(|(mut choices, level)| choices.swap_remove(level))
        .collect()
}
