//! What a Browser tab's page may use of the camera and microphone beyond the site's saved
//! choices (Settings › Browser › Site settings): the "Allow while visiting" answers to the tab's
//! own question. Such an answer lasts while that tab shows that site: it ends when the tab goes
//! to another site or closes, and with the app (it is never saved). "Allow this time" answers
//! one request only, and "Never" is saved as the site's choice, so neither is kept here.

use std::collections::HashMap;

use brigadier_ipc::app::SitePermission;

use crate::browser_store::Device;

/// How a page's request for devices goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Grant,
    Deny,
    /// The tab asks the user.
    Ask,
}

/// The answer from the site's saved choice for the devices (`browser_store::combined`) and
/// whether the tab already allows them while visiting.
pub fn answer(saved: SitePermission, visiting: bool) -> Answer {
    match saved {
        SitePermission::Allow => Answer::Grant,
        SitePermission::Block => Answer::Deny,
        SitePermission::Ask if visiting => Answer::Grant,
        SitePermission::Ask => Answer::Ask,
    }
}

/// The site (`scheme://host[:port]`) a page's address belongs to.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    (!host.is_empty()).then(|| {
        format!(
            "{}://{}",
            scheme.to_ascii_lowercase(),
            host.to_ascii_lowercase()
        )
    })
}

/// The "while visiting" grants, by tab: the site and the devices it may use.
#[derive(Default)]
pub struct Visiting {
    tabs: HashMap<String, (String, Vec<Device>)>,
}

impl Visiting {
    /// Whether tab `tab`, showing `origin`, may use all of `devices` now.
    pub fn allows(&self, tab: &str, origin: &str, devices: &[Device]) -> bool {
        self.tabs.get(tab).is_some_and(|(site, allowed)| {
            site == origin && devices.iter().all(|device| allowed.contains(device))
        })
    }

    /// Tab `tab` may use `devices` while it shows `origin`.
    pub fn grant(&mut self, tab: &str, origin: &str, devices: &[Device]) {
        let entry = self
            .tabs
            .entry(tab.to_owned())
            .or_insert_with(|| (origin.to_owned(), Vec::new()));
        if entry.0 != origin {
            *entry = (origin.to_owned(), Vec::new());
        }
        for device in devices {
            if !entry.1.contains(device) {
                entry.1.push(*device);
            }
        }
    }

    /// Tab `tab` started loading `url`: on another site its grants end.
    pub fn navigated(&mut self, tab: &str, url: &str) {
        let same = self
            .tabs
            .get(tab)
            .is_some_and(|(site, _)| origin_of(url).as_deref() == Some(site));
        if !same {
            self.tabs.remove(tab);
        }
    }

    /// Tab `tab` closed: its grants end.
    pub fn closed(&mut self, tab: &str) {
        self.tabs.remove(tab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "https://meet.example";

    #[test]
    fn saved_choices_answer_first_and_ask_waits_for_the_tab() {
        assert_eq!(answer(SitePermission::Allow, false), Answer::Grant);
        assert_eq!(
            answer(SitePermission::Block, true),
            Answer::Deny,
            "Never beats a visit"
        );
        assert_eq!(answer(SitePermission::Ask, true), Answer::Grant);
        assert_eq!(answer(SitePermission::Ask, false), Answer::Ask);
    }

    #[test]
    fn a_site_is_its_scheme_host_and_port() {
        assert_eq!(
            origin_of("https://Meet.Example/room?x=1#y").as_deref(),
            Some(SITE)
        );
        assert_eq!(
            origin_of("http://localhost:8080/").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(
            origin_of("https://user@meet.example/").as_deref(),
            Some(SITE)
        );
        assert_eq!(origin_of("about:blank"), None);
    }

    #[test]
    fn allow_while_visiting_lasts_on_the_site_in_that_tab_only() {
        let mut visiting = Visiting::default();
        visiting.grant("tab", SITE, &[Device::Camera]);
        assert!(visiting.allows("tab", SITE, &[Device::Camera]));
        assert!(
            !visiting.allows("tab", SITE, &[Device::Camera, Device::Microphone]),
            "only what was asked"
        );
        assert!(
            !visiting.allows("other", SITE, &[Device::Camera]),
            "another tab asks again"
        );
        assert!(!visiting.allows("tab", "https://other.example", &[Device::Camera]));
        visiting.grant("tab", SITE, &[Device::Microphone]);
        assert!(visiting.allows("tab", SITE, &[Device::Camera, Device::Microphone]));
    }

    #[test]
    fn allow_while_visiting_ends_on_another_site_but_not_another_page() {
        let mut visiting = Visiting::default();
        visiting.grant("tab", SITE, &[Device::Camera]);
        visiting.navigated("tab", "https://meet.example/another-room");
        assert!(
            visiting.allows("tab", SITE, &[Device::Camera]),
            "a page of the same site"
        );
        visiting.navigated("tab", "https://other.example/");
        assert!(!visiting.allows("tab", SITE, &[Device::Camera]));
        visiting.navigated("tab", "https://meet.example/");
        assert!(
            !visiting.allows("tab", SITE, &[Device::Camera]),
            "coming back asks again"
        );
    }

    #[test]
    fn allow_while_visiting_ends_when_the_tab_closes_and_with_the_app() {
        let mut visiting = Visiting::default();
        visiting.grant("tab", SITE, &[Device::Camera]);
        visiting.grant("other", SITE, &[Device::Camera]);
        visiting.closed("tab");
        assert!(!visiting.allows("tab", SITE, &[Device::Camera]));
        assert!(visiting.allows("other", SITE, &[Device::Camera]));
        // Nothing is saved: the app starting again starts with none.
        assert!(!Visiting::default().allows("other", SITE, &[Device::Camera]));
    }
}
