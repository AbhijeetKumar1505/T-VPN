//! Friendly names for AWS regions: `tvpn cloud up mumbai`, `tokyo`, `de`, `uk`...

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// AWS region code, e.g. `ap-south-1`.
    pub code: String,
    pub city: String,
    pub country: String,
    /// Opt-in regions must be enabled in the AWS account first.
    pub opt_in: bool,
}

struct Entry {
    code: &'static str,
    city: &'static str,
    country: &'static str,
    opt_in: bool,
    /// Lowercase aliases: city names and ISO country codes.
    aliases: &'static [&'static str],
}

const REGIONS: &[Entry] = &[
    Entry {
        code: "us-east-1",
        city: "N. Virginia",
        country: "United States",
        opt_in: false,
        aliases: &["virginia", "us", "usa", "us-east"],
    },
    Entry { code: "us-east-2", city: "Ohio", country: "United States", opt_in: false, aliases: &["ohio"] },
    Entry {
        code: "us-west-1",
        city: "N. California",
        country: "United States",
        opt_in: false,
        aliases: &["california", "sf", "us-west"],
    },
    Entry {
        code: "us-west-2",
        city: "Oregon",
        country: "United States",
        opt_in: false,
        aliases: &["oregon"],
    },
    Entry {
        code: "ca-central-1",
        city: "Montreal",
        country: "Canada",
        opt_in: false,
        aliases: &["canada", "montreal", "ca"],
    },
    Entry { code: "ca-west-1", city: "Calgary", country: "Canada", opt_in: true, aliases: &["calgary"] },
    Entry {
        code: "sa-east-1",
        city: "São Paulo",
        country: "Brazil",
        opt_in: false,
        aliases: &["saopaulo", "sao-paulo", "brazil", "br"],
    },
    Entry {
        code: "eu-west-1",
        city: "Dublin",
        country: "Ireland",
        opt_in: false,
        aliases: &["ireland", "dublin", "ie"],
    },
    Entry {
        code: "eu-west-2",
        city: "London",
        country: "United Kingdom",
        opt_in: false,
        aliases: &["london", "uk", "gb"],
    },
    Entry {
        code: "eu-west-3",
        city: "Paris",
        country: "France",
        opt_in: false,
        aliases: &["paris", "france", "fr"],
    },
    Entry {
        code: "eu-central-1",
        city: "Frankfurt",
        country: "Germany",
        opt_in: false,
        aliases: &["frankfurt", "germany", "de"],
    },
    Entry {
        code: "eu-central-2",
        city: "Zurich",
        country: "Switzerland",
        opt_in: true,
        aliases: &["zurich", "switzerland", "ch"],
    },
    Entry {
        code: "eu-north-1",
        city: "Stockholm",
        country: "Sweden",
        opt_in: false,
        aliases: &["stockholm", "sweden", "se"],
    },
    Entry {
        code: "eu-south-1",
        city: "Milan",
        country: "Italy",
        opt_in: true,
        aliases: &["milan", "italy", "it"],
    },
    Entry {
        code: "eu-south-2",
        city: "Aragon",
        country: "Spain",
        opt_in: true,
        aliases: &["spain", "es", "aragon"],
    },
    Entry {
        code: "ap-south-1",
        city: "Mumbai",
        country: "India",
        opt_in: false,
        aliases: &["mumbai", "india", "in"],
    },
    Entry { code: "ap-south-2", city: "Hyderabad", country: "India", opt_in: true, aliases: &["hyderabad"] },
    Entry {
        code: "ap-northeast-1",
        city: "Tokyo",
        country: "Japan",
        opt_in: false,
        aliases: &["tokyo", "japan", "jp"],
    },
    Entry {
        code: "ap-northeast-2",
        city: "Seoul",
        country: "South Korea",
        opt_in: false,
        aliases: &["seoul", "korea", "kr"],
    },
    Entry { code: "ap-northeast-3", city: "Osaka", country: "Japan", opt_in: false, aliases: &["osaka"] },
    Entry {
        code: "ap-southeast-1",
        city: "Singapore",
        country: "Singapore",
        opt_in: false,
        aliases: &["singapore", "sg"],
    },
    Entry {
        code: "ap-southeast-2",
        city: "Sydney",
        country: "Australia",
        opt_in: false,
        aliases: &["sydney", "australia", "au"],
    },
    Entry {
        code: "ap-southeast-3",
        city: "Jakarta",
        country: "Indonesia",
        opt_in: true,
        aliases: &["jakarta", "indonesia", "id"],
    },
    Entry {
        code: "ap-southeast-4",
        city: "Melbourne",
        country: "Australia",
        opt_in: true,
        aliases: &["melbourne"],
    },
    Entry {
        code: "ap-east-1",
        city: "Hong Kong",
        country: "Hong Kong",
        opt_in: true,
        aliases: &["hongkong", "hong-kong", "hk"],
    },
    Entry {
        code: "me-south-1",
        city: "Bahrain",
        country: "Bahrain",
        opt_in: true,
        aliases: &["bahrain", "bh"],
    },
    Entry {
        code: "me-central-1",
        city: "UAE",
        country: "United Arab Emirates",
        opt_in: true,
        aliases: &["uae", "dubai", "ae"],
    },
    Entry {
        code: "il-central-1",
        city: "Tel Aviv",
        country: "Israel",
        opt_in: true,
        aliases: &["telaviv", "tel-aviv", "israel", "il"],
    },
    Entry {
        code: "af-south-1",
        city: "Cape Town",
        country: "South Africa",
        opt_in: true,
        aliases: &["capetown", "cape-town", "southafrica", "za"],
    },
];

fn to_region(e: &Entry) -> Region {
    Region { code: e.code.into(), city: e.city.into(), country: e.country.into(), opt_in: e.opt_in }
}

pub fn all() -> Vec<Region> {
    REGIONS.iter().map(to_region).collect()
}

/// Regions enabled in every AWS account (used when scanning for servers).
pub fn default_enabled() -> Vec<Region> {
    REGIONS.iter().filter(|e| !e.opt_in).map(to_region).collect()
}

/// Shortest friendly name for a region code (for display and session names).
pub fn short_name(code: &str) -> String {
    REGIONS
        .iter()
        .find(|e| e.code == code)
        .and_then(|e| e.aliases.first())
        .map(|a| a.to_string())
        .unwrap_or_else(|| code.to_string())
}

fn looks_like_region_code(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() >= 3
        && parts[0].len() == 2
        && parts.last().is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Resolve a city, country code, or AWS region code.
pub fn resolve(input: &str) -> Result<Region> {
    let key: String = input.trim().to_lowercase().replace([' ', '_'], "-");
    let squashed = key.replace('-', "");
    for e in REGIONS {
        if e.code == key
            || e.aliases.iter().any(|a| *a == key || *a == squashed)
            || e.city.to_lowercase() == input.trim().to_lowercase()
        {
            return Ok(to_region(e));
        }
    }
    if looks_like_region_code(&key) {
        // A region tvpn doesn't know by name yet; let AWS decide.
        return Ok(Region { code: key.clone(), city: key, country: String::new(), opt_in: false });
    }
    let prefix: String = squashed.chars().take(2).collect();
    let suggestions: Vec<&str> = REGIONS
        .iter()
        .filter(|e| !prefix.is_empty() && e.aliases.iter().any(|a| a.starts_with(&prefix)))
        .filter_map(|e| e.aliases.first().copied())
        .take(5)
        .collect();
    if suggestions.is_empty() {
        bail!("unknown region `{input}`. See `tvpn cloud regions` for the list");
    }
    bail!("unknown region `{input}`. Did you mean: {}? (see `tvpn cloud regions`)", suggestions.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_names_codes_and_countries() {
        assert_eq!(resolve("mumbai").unwrap().code, "ap-south-1");
        assert_eq!(resolve("Mumbai").unwrap().code, "ap-south-1");
        assert_eq!(resolve("in").unwrap().code, "ap-south-1");
        assert_eq!(resolve("UK").unwrap().code, "eu-west-2");
        assert_eq!(resolve("de").unwrap().code, "eu-central-1");
        assert_eq!(resolve("hong kong").unwrap().code, "ap-east-1");
        assert_eq!(resolve("São Paulo").unwrap().code, "sa-east-1");
        assert_eq!(resolve("ap-northeast-1").unwrap().city, "Tokyo");
        assert!(resolve("hyderabad").unwrap().opt_in);
    }

    #[test]
    fn unknown_codes_pass_through_and_typos_fail() {
        assert_eq!(resolve("xx-newplace-9").unwrap().code, "xx-newplace-9");
        let err = resolve("mumbia").unwrap_err().to_string();
        assert!(err.contains("Did you mean"), "{err}");
        assert!(resolve("zzz").is_err());
    }

    #[test]
    fn table_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for e in REGIONS {
            assert!(looks_like_region_code(e.code), "{}", e.code);
            assert!(seen.insert(e.code), "duplicate {}", e.code);
            for a in e.aliases {
                assert_eq!(resolve(a).unwrap().code, e.code, "alias {a}");
            }
        }
        assert_eq!(short_name("eu-west-2"), "london");
    }
}
