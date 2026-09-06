//! Attribution for the Mapillary imagery a generation used.
//!
//! Mapillary imagery is CC BY-SA 4.0, and the licence is on the pixels: a world
//! built from it carries the obligation to name the photographer. Mapillary's
//! own guidance asks for one line per image, shaped
//!
//!     "Title" [link to the image] by "username" [link to the profile], licensed under CC-BY-SA
//!
//! so their example reads `[Madeira, Portugal] by [nunocaldeira], licensed
//! under CC-BY-SA`.
//!
//! Every image whose pixels reach a wall is recorded here as it is used, and
//! the GUI appends the list to License and Credits while the CLI prints it at
//! the end of a run. The store is process wide and replaced per generation, the
//! same way the facade store is, because the GUI builds many worlds in one
//! process.

use std::collections::BTreeMap;
use std::sync::RwLock;

/// One image, as it should be credited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageCredit {
    /// Mapillary image id, which is also its permalink.
    pub id: String,
    /// What to show as the title. Mapillary has no title field on an image, so
    /// this is the best available description of where it was taken, and falls
    /// back to the image id when there is nothing better.
    pub title: String,
    /// The uploader's username, as Mapillary shows it.
    pub username: String,
    /// The uploader's numeric id, for the profile link.
    pub user_id: String,
}

/// What to call an uploader an export carries no name or id for. The credit
/// still names the photograph and links to it, which is where Mapillary shows
/// who took it; only the name is missing, and saying so is better than
/// crediting "unknown" as though that were a person.
pub const UNNAMED: &str = "an uploader this facade export does not name";

impl ImageCredit {
    /// An image known only by its id, which is what a facade folder built
    /// outside this process carries: the per wall JSON lists the images that
    /// reached the wall, and nothing about who uploaded them.
    pub fn by_id(id: &str) -> Self {
        Self {
            id: id.to_string(),
            title: id.to_string(),
            username: String::new(),
            user_id: String::new(),
        }
    }

    /// Whether the uploader can be pointed at, by name or by numeric id.
    pub fn names_the_uploader(&self) -> bool {
        !self.username.is_empty() || !self.user_id.is_empty()
    }

    /// `https://www.mapillary.com/app/?pKey=<id>&focus=photo`, the link that
    /// opens this exact photograph.
    pub fn image_url(&self) -> String {
        format!(
            "https://www.mapillary.com/app/?pKey={}&focus=photo",
            self.id
        )
    }

    /// The uploader's profile. Mapillary routes both the username and the
    /// numeric id; the username is the stable public one. Empty when the
    /// export named neither, because a link to a profile route with nothing
    /// on the end of it goes nowhere.
    pub fn profile_url(&self) -> String {
        if !self.names_the_uploader() {
            return String::new();
        }
        if self.username.is_empty() {
            format!("https://www.mapillary.com/app/user/{}", self.user_id)
        } else {
            format!("https://www.mapillary.com/app/user/{}", self.username)
        }
    }

    /// What to show where the username goes.
    pub fn uploader(&self) -> &str {
        if !self.username.is_empty() {
            &self.username
        } else if !self.user_id.is_empty() {
            "unknown"
        } else {
            UNNAMED
        }
    }

    /// The credit as one plain text line, for the CLI and for logs.
    pub fn line(&self) -> String {
        if !self.names_the_uploader() {
            return format!(
                "{} ({}) by {UNNAMED}, licensed under CC-BY-SA",
                self.title,
                self.image_url()
            );
        }
        format!(
            "{} ({}) by {} ({}), licensed under CC-BY-SA",
            self.title,
            self.image_url(),
            self.uploader(),
            self.profile_url()
        )
    }
}

/// Replaced on every generation: a second world in the same process must not
/// inherit the first world's credits.
static CREDITS: RwLock<BTreeMap<String, ImageCredit>> = RwLock::new(BTreeMap::new());

/// Forgets everything recorded so far. Called when a generation starts.
pub fn reset() {
    CREDITS.write().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Records one image. Recording the same id twice keeps the first entry, so a
/// panorama used by twenty walls is credited once.
pub fn record(credit: ImageCredit) {
    if credit.id.is_empty() {
        return;
    }
    CREDITS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .entry(credit.id.clone())
        .or_insert(credit);
}

/// Every image used, ordered by id so two runs of the same area produce the
/// same list.
pub fn list() -> Vec<ImageCredit> {
    CREDITS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect()
}

/// How many images are credited.
pub fn count() -> usize {
    CREDITS.read().unwrap_or_else(|e| e.into_inner()).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The store is process wide, so tests that touch it take turns.
    static GLOBALS: Mutex<()> = Mutex::new(());

    fn credit(id: &str, user: &str) -> ImageCredit {
        ImageCredit {
            id: id.to_string(),
            title: "Ledererstrasse, Munich".to_string(),
            username: user.to_string(),
            user_id: "42".to_string(),
        }
    }

    #[test]
    fn a_credit_line_has_the_shape_mapillary_asks_for() {
        let c = credit("1234", "nunocaldeira");
        assert_eq!(
            c.line(),
            "Ledererstrasse, Munich (https://www.mapillary.com/app/?pKey=1234&focus=photo) \
             by nunocaldeira (https://www.mapillary.com/app/user/nunocaldeira), \
             licensed under CC-BY-SA"
        );
    }

    #[test]
    fn an_image_used_twice_is_credited_once_and_the_list_is_stable() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        record(credit("2", "bob"));
        record(credit("1", "alice"));
        record(credit("2", "someone else"));
        let all = list();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "1");
        assert_eq!(all[1].username, "bob", "the first record of an id wins");
        assert_eq!(count(), 2);
    }

    #[test]
    fn a_missing_username_still_links_to_the_uploader() {
        let c = ImageCredit {
            username: String::new(),
            ..credit("9", "")
        };
        assert_eq!(c.profile_url(), "https://www.mapillary.com/app/user/42");
        assert!(c.line().contains("by unknown"));
    }

    #[test]
    fn an_image_known_only_by_its_id_still_credits_the_photograph() {
        // What a facade folder built outside this process can say: which
        // photograph, and where to find it. The uploader's name is on that
        // page, and the credit says why it is not here.
        let c = ImageCredit::by_id("1071917607838019");
        assert!(!c.names_the_uploader());
        assert_eq!(c.profile_url(), "", "no link that goes nowhere");
        assert_eq!(
            c.line(),
            "1071917607838019 (https://www.mapillary.com/app/?pKey=1071917607838019&focus=photo) \
             by an uploader this facade export does not name, licensed under CC-BY-SA"
        );
    }

    #[test]
    fn a_generation_does_not_inherit_the_previous_worlds_credits() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        record(credit("1", "alice"));
        reset();
        assert!(list().is_empty());
    }
}
