use crate::model::Identity;
use serde::Deserialize;
use std::{collections::BTreeMap, fs, io::ErrorKind, path::Path};

#[derive(Default)]
pub struct Config {
    aliases: BTreeMap<String, (String, String)>,
    pub timezone: Option<String>,
    pub exclude: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    contributors: BTreeMap<String, Vec<String>>,
    timezone: Option<String>,
    exclude: Vec<String>,
}

impl Config {
    pub fn canonical_author_id(&self, id: &str) -> String {
        let id = id.trim().to_lowercase();
        self.aliases
            .get(&id)
            .map(|(canonical, _)| canonical.clone())
            .unwrap_or(id)
    }

    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join(".git-wrapped.json");
        let contents = match fs::read(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let file: ConfigFile = serde_json::from_slice(&contents)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut config = Self {
            timezone: file.timezone,
            exclude: file.exclude,
            ..Self::default()
        };
        for (name, addresses) in file.contributors {
            config
                .insert_alias_group(
                    &name,
                    &addresses.iter().map(String::as_str).collect::<Vec<_>>(),
                )
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }
        Ok(config)
    }

    pub fn insert_alias_group(&mut self, name: &str, addresses: &[&str]) -> Result<(), String> {
        if name.trim().is_empty() || addresses.is_empty() {
            return Err("contributor name and addresses must not be blank".into());
        }
        let emails = addresses
            .iter()
            .map(|address| address.trim().to_ascii_lowercase())
            .collect::<Vec<_>>();
        if emails.iter().any(String::is_empty) {
            return Err("contributor address must not be blank".into());
        }
        for email in &emails {
            if let Some((_, existing_name)) = self.aliases.get(email) {
                if existing_name != name {
                    return Err(format!("address {email} belongs to multiple contributors"));
                }
            }
        }
        let Some(id) = self
            .aliases
            .iter()
            .filter_map(|(email, (_, existing_name))| (existing_name == name).then_some(email))
            .chain(emails.iter())
            .min()
            .cloned()
        else {
            return Err("contributor address must not be blank".into());
        };
        for (existing_id, existing_name) in self.aliases.values_mut() {
            if existing_name == name {
                *existing_id = id.clone();
            }
        }
        for email in emails {
            self.aliases.insert(email, (id.clone(), name.to_owned()));
        }
        Ok(())
    }
}

pub fn normalize(mapped: &Identity, config: &Config) -> (String, String) {
    let email = mapped.email.trim().to_ascii_lowercase();
    if let Some((id, name)) = config.aliases.get(&email) {
        return (id.clone(), name.clone());
    }
    if email.is_empty() {
        (
            format!("name:{}", mapped.name.trim().to_lowercase()),
            mapped.name.clone(),
        )
    } else {
        (email, mapped.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_share_one_stable_id() {
        let mut config = Config::default();
        config
            .insert_alias_group("Team Member", &["correct@example.com", "OTHER@EXAMPLE.COM"])
            .unwrap();
        let first = Identity {
            name: "Correct Name".into(),
            email: "correct@example.com".into(),
        };
        let second = Identity {
            name: "Other".into(),
            email: "other@example.com".into(),
        };
        assert_eq!(
            normalize(&first, &config),
            ("correct@example.com".into(), "Team Member".into())
        );
        assert_eq!(normalize(&second, &config), normalize(&first, &config));
    }

    #[test]
    fn rejects_case_folded_alias_collision() {
        let mut config = Config::default();
        config
            .insert_alias_group("One", &["A@EXAMPLE.COM"])
            .unwrap();
        assert!(config
            .insert_alias_group("Two", &["a@example.com"])
            .is_err());
    }
}
