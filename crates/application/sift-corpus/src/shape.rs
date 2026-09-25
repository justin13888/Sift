//! The shape of the corpus, and how it is divided between accounts and folders.
//!
//! The reference environment fixes three numbers — about five accounts, 500,000 messages, 50,000
//! of them in an inbox — and says "the shape matters more than the content". Everything else
//! here is a division of those three numbers, stated so that two runs describe the same corpus.

use sift_provider::adapter::SpecialUse;

/// The three numbers the reference environment fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub accounts: u16,
    pub messages: u64,
    /// The size of **one** inbox, the primary account's. NFR-1's first screen and the
    /// unified inbox's merge are both paid against the largest folder a person has, and a
    /// population spread thinly across five inboxes would measure neither.
    pub inbox: u64,
}

impl Shape {
    /// The scale corpus of `docs/product/reference-environment.md`.
    pub const SCALE: Self = Self {
        accounts: 5,
        messages: 500_000,
        inbox: 50_000,
    };

    /// The same proportions, `divisor` times smaller — for a smoke run, and for tests.
    ///
    /// # Errors
    /// A divisor of zero.
    pub fn divided(self, divisor: u64) -> Result<Self, ShapeError> {
        if divisor == 0 {
            return Err(ShapeError::ZeroDivisor);
        }
        Ok(Self {
            accounts: self.accounts,
            messages: self.messages / divisor,
            inbox: self.inbox / divisor,
        })
    }

    /// Divide the population between accounts and their folders.
    ///
    /// # Errors
    /// No accounts, fewer messages than accounts, or an inbox larger than the primary
    /// account's share can hold.
    pub fn plan(self) -> Result<Vec<AccountPlan>, ShapeError> {
        if self.accounts == 0 {
            return Err(ShapeError::NoAccounts);
        }
        let shares = shares(self.messages, self.accounts);
        if shares.contains(&0) {
            return Err(ShapeError::TooFewMessages {
                messages: self.messages,
                accounts: self.accounts,
            });
        }
        let primary = shares[0];
        if self.inbox > primary {
            return Err(ShapeError::InboxTooLarge {
                inbox: self.inbox,
                primary,
            });
        }
        Ok(shares
            .iter()
            .enumerate()
            .map(|(i, total)| {
                let inbox = if i == 0 {
                    self.inbox
                } else {
                    total * SECONDARY_INBOX_PERCENT / 100
                };
                AccountPlan {
                    index: i,
                    display_name: format!("Corpus {}", i + 1),
                    address: format!("owner{}@corpus-{}.test", i + 1, i + 1),
                    folders: folders(*total, inbox),
                }
            })
            .collect())
    }
}

/// A secondary account's inbox, as a share of its own mail. Small, because the one large
/// inbox is the measurement and the others are the population it is merged against.
const SECONDARY_INBOX_PERCENT: u64 = 2;

/// Why a shape cannot be planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeError {
    ZeroDivisor,
    NoAccounts,
    TooFewMessages { messages: u64, accounts: u16 },
    InboxTooLarge { inbox: u64, primary: u64 },
}

impl core::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ZeroDivisor => write!(f, "a divisor of zero divides nothing"),
            Self::NoAccounts => write!(f, "a corpus needs at least one account"),
            Self::TooFewMessages { messages, accounts } => write!(
                f,
                "{messages} messages leave at least one of {accounts} accounts empty"
            ),
            Self::InboxTooLarge { inbox, primary } => write!(
                f,
                "an inbox of {inbox} does not fit the primary account's {primary} messages"
            ),
        }
    }
}

impl std::error::Error for ShapeError {}

/// One account's slice of the corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPlan {
    pub index: usize,
    pub display_name: String,
    /// The address the account's mail is to — and from, in its sent folder.
    pub address: String,
    pub folders: Vec<FolderPlan>,
}

impl AccountPlan {
    #[must_use]
    pub fn messages(&self) -> u64 {
        self.folders.iter().map(|f| f.messages).sum()
    }

    #[must_use]
    pub fn inbox(&self) -> u64 {
        self.folders
            .iter()
            .find(|f| f.special_use == Some(SpecialUse::Inbox))
            .map_or(0, |f| f.messages)
    }
}

/// One folder, and how many messages it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPlan {
    pub remote_id: String,
    pub display_name: String,
    pub special_use: Option<SpecialUse>,
    pub messages: u64,
}

/// Linearly descending shares — five accounts are 5:4:3:2:1 — summing exactly to `messages`.
///
/// Descending rather than equal because a person's accounts are not equal: one carries most of
/// their mail, and the index merge D-42 pays on every keystroke is dominated by the largest.
fn shares(messages: u64, accounts: u16) -> Vec<u64> {
    let n = u64::from(accounts);
    let weight_sum = n * (n + 1) / 2;
    let mut out: Vec<u64> = (0..n).map(|i| messages * (n - i) / weight_sum).collect();
    let assigned: u64 = out.iter().sum();
    // What integer division left over goes to the primary, so the total is exact.
    out[0] += messages - assigned;
    out
}

/// The folder layout of one account: the inbox as planned, special-use folders at fixed shares,
/// and the rest in an archive and six user folders of descending size.
fn folders(total: u64, inbox: u64) -> Vec<FolderPlan> {
    let rest = total - inbox;
    let sent = (total / 10).min(rest);
    let trash = (total / 50).min(rest - sent);
    let spam = (total / 100).min(rest - sent - trash);
    let filed = rest - sent - trash - spam;

    let mut out = vec![
        special("INBOX", "Inbox", SpecialUse::Inbox, inbox),
        special("SENT", "Sent", SpecialUse::Sent, sent),
        special("TRASH", "Trash", SpecialUse::Trash, trash),
        special("SPAM", "Junk", SpecialUse::Spam, spam),
    ];

    // Archive takes six tenths of what is filed; the user folders halve down from the rest.
    let archive = filed * 6 / 10;
    let mut remaining = filed - archive;
    out.push(special("ARCHIVE", "Archive", SpecialUse::Archive, archive));
    for (i, name) in USER_FOLDERS.iter().enumerate() {
        let count = if i + 1 == USER_FOLDERS.len() {
            remaining
        } else {
            remaining / 2
        };
        remaining -= count;
        out.push(FolderPlan {
            remote_id: format!("F{}", i + 1),
            display_name: (*name).to_owned(),
            special_use: None,
            messages: count,
        });
    }
    out
}

const USER_FOLDERS: &[&str] = &[
    "Receipts",
    "Projects",
    "Travel",
    "Family",
    "Newsletters",
    "Ünterlagen",
];

fn special(remote: &str, name: &str, use_: SpecialUse, messages: u64) -> FolderPlan {
    FolderPlan {
        remote_id: remote.to_owned(),
        display_name: name.to_owned(),
        special_use: Some(use_),
        messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_corpus_is_the_shape_the_reference_environment_fixes() {
        let plan = Shape::SCALE.plan().expect("plan");
        assert_eq!(plan.len(), 5, "five accounts");
        assert_eq!(
            plan.iter().map(AccountPlan::messages).sum::<u64>(),
            500_000,
            "500,000 messages, exactly"
        );
        assert_eq!(plan[0].inbox(), 50_000, "one 50,000-message inbox");
        assert!(
            plan.iter().skip(1).all(|a| a.inbox() < 50_000),
            "the primary inbox is the large one"
        );
    }

    #[test]
    fn every_account_and_every_folder_sums_exactly() {
        for shape in [
            Shape::SCALE,
            Shape::SCALE.divided(1000).expect("divide"),
            Shape {
                accounts: 3,
                messages: 1_001,
                inbox: 17,
            },
            Shape {
                accounts: 1,
                messages: 7,
                inbox: 7,
            },
        ] {
            let plan = shape.plan().expect("plan");
            assert_eq!(
                plan.iter().map(AccountPlan::messages).sum::<u64>(),
                shape.messages
            );
            assert_eq!(plan[0].inbox(), shape.inbox);
        }
    }

    #[test]
    fn shares_descend() {
        let plan = Shape::SCALE.plan().expect("plan");
        for pair in plan.windows(2) {
            assert!(pair[0].messages() > pair[1].messages());
        }
    }

    #[test]
    fn a_shape_that_cannot_be_planned_says_why() {
        assert_eq!(
            Shape::SCALE.divided(0).unwrap_err(),
            ShapeError::ZeroDivisor
        );
        assert_eq!(
            Shape {
                accounts: 0,
                messages: 1,
                inbox: 0
            }
            .plan()
            .unwrap_err(),
            ShapeError::NoAccounts
        );
        assert!(matches!(
            Shape {
                accounts: 5,
                messages: 3,
                inbox: 0
            }
            .plan(),
            Err(ShapeError::TooFewMessages { .. })
        ));
        assert!(matches!(
            Shape {
                accounts: 5,
                messages: 100,
                inbox: 90
            }
            .plan(),
            Err(ShapeError::InboxTooLarge { .. })
        ));
    }

    #[test]
    fn every_account_has_every_special_use_folder_once() {
        for account in Shape::SCALE.plan().expect("plan") {
            for use_ in [
                SpecialUse::Inbox,
                SpecialUse::Sent,
                SpecialUse::Trash,
                SpecialUse::Spam,
                SpecialUse::Archive,
            ] {
                assert_eq!(
                    account
                        .folders
                        .iter()
                        .filter(|f| f.special_use == Some(use_))
                        .count(),
                    1
                );
            }
        }
    }
}
