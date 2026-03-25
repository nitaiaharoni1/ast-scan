//! Heuristic detection of hardcoded secrets in string literals.

use std::sync::OnceLock;

use regex::Regex;

use crate::types::SecurityFinding;

fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let len = s.len() as f64;
    let mut h = 0.0;
    for &c in counts.iter() {
        if c == 0 {
            continue;
        }
        let p = c as f64 / len;
        h -= p * p.log2();
    }
    h
}

fn re_stripe() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^sk_live_[0-9a-zA-Z]{20,}$").expect("stripe regex"))
}

fn re_aws() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^AKIA[0-9A-Z]{16}$").expect("aws regex"))
}

fn re_github() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^(gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})$")
            .expect("github regex")
    })
}

fn re_jwt() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}$")
            .expect("jwt regex")
    })
}

fn re_slack() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^xox[baprs]-[0-9A-Za-z-]{10,}$").expect("slack regex")
    })
}

fn re_google_api() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^AIza[0-9A-Za-z_-]{35}$").expect("google api regex")
    })
}

fn re_sendgrid() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}$").expect("sendgrid regex")
    })
}

fn re_pem_header() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // No anchors: PEM header may be embedded in a multiline string literal
        Regex::new(r"-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----")
            .expect("pem regex")
    })
}

fn re_db_url() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // No ^ anchor: connection string may appear inside a longer string value
        Regex::new(r"(postgres|mysql|mongodb)://[^:@\s]{1,64}:[^@\s]{1,128}@")
            .expect("db url regex")
    })
}

fn re_heroku() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
            .expect("heroku regex")
    })
}

fn re_mailgun() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^key-[0-9a-zA-Z]{32}$").expect("mailgun regex")
    })
}

fn re_twilio() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^AC[0-9a-fA-F]{32}$").expect("twilio regex")
    })
}

/// Inspect a string literal; returns a finding if it looks like a credential.
pub(crate) fn audit_string_literal(
    value: &str,
    file: &str,
    line: usize,
    context_hint: &str,
) -> Option<SecurityFinding> {
    let trimmed = value.trim();
    if trimmed.len() < 8 {
        return None;
    }

    if re_stripe().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "stripe_live_key".into(),
            file: file.into(),
            line,
            detail: "Possible Stripe live secret key pattern".into(),
        });
    }
    if re_aws().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "aws_access_key".into(),
            file: file.into(),
            line,
            detail: "Possible AWS access key id (AKIA...)".into(),
        });
    }
    if re_github().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "github_token".into(),
            file: file.into(),
            line,
            detail: "Possible GitHub personal access token pattern".into(),
        });
    }
    if re_jwt().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "jwt_token".into(),
            file: file.into(),
            line,
            detail: "Possible hardcoded JWT token".into(),
        });
    }
    if re_slack().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "slack_token".into(),
            file: file.into(),
            line,
            detail: "Possible Slack API token (xox...)".into(),
        });
    }
    if re_google_api().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "google_api_key".into(),
            file: file.into(),
            line,
            detail: "Possible Google API key (AIza...)".into(),
        });
    }
    if re_sendgrid().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "sendgrid_api_key".into(),
            file: file.into(),
            line,
            detail: "Possible SendGrid API key (SG....)".into(),
        });
    }
    if re_pem_header().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "private_key_pem".into(),
            file: file.into(),
            line,
            detail: "Possible PEM private key header in string literal".into(),
        });
    }
    if re_db_url().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "db_connection_string".into(),
            file: file.into(),
            line,
            detail: "Possible database URL with embedded credentials".into(),
        });
    }
    if re_heroku().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "heroku_api_key".into(),
            file: file.into(),
            line,
            detail: "Possible Heroku API key (UUID pattern)".into(),
        });
    }
    if re_mailgun().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "mailgun_api_key".into(),
            file: file.into(),
            line,
            detail: "Possible Mailgun API key (key-...)".into(),
        });
    }
    if re_twilio().is_match(trimmed) {
        return Some(SecurityFinding {
            kind: "twilio_sid".into(),
            file: file.into(),
            line,
            detail: "Possible Twilio Account SID (AC...)".into(),
        });
    }

    let ctx = context_hint.to_ascii_lowercase();
    let sensitive_name = ctx.contains("password")
        || ctx.contains("passwd")
        || ctx.contains("secret")
        || ctx.contains("token")
        || ctx.contains("apikey")
        || ctx.contains("api_key")
        || ctx.contains("auth");

    if sensitive_name && trimmed.len() >= 12 {
        let ent = shannon_entropy(trimmed);
        if ent > 3.5 && trimmed.chars().all(|c| c.is_ascii_graphic()) {
            return Some(SecurityFinding {
                kind: "high_entropy_assignment".into(),
                file: file.into(),
                line,
                detail: format!(
                    "High-entropy string near sensitive name ({context_hint}); verify not a secret"
                ),
            });
        }
    }

    if trimmed.len() >= 32
        && trimmed.chars().all(|c| c.is_ascii_hexdigit())
        && shannon_entropy(trimmed) > 3.0
    {
        return Some(SecurityFinding {
            kind: "long_hex_literal".into(),
            file: file.into(),
            line,
            detail: "Long hex string; could be a key or hash".into(),
        });
    }

    None
}
