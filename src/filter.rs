use crate::config::FilterConfig;
use grammers_client::message::Message;
use grammers_tl_types as tl;

pub trait MessageFilter: Send + Sync {
    fn accept(&self, msg: &Message) -> bool;
}

pub struct ChannelFilter {
    pub mode: crate::config::FilterMode,
    pub filters: Vec<Box<dyn MessageFilter>>,
}

impl ChannelFilter {
    pub fn new(config: &FilterConfig) -> Self {
        let mut filters: Vec<Box<dyn MessageFilter>> = Vec::new();

        if let Some(reactions_config) = &config.reactions {
            if !reactions_config.specific.is_empty() || reactions_config.min_specific_total > 0 {
                filters.push(Box::new(ReactionFilter {
                    specific: reactions_config.specific.iter().map(|s| normalize_emoji(s)).collect(),
                    min_specific_total: reactions_config.min_specific_total,
                }));
            }
        }

        if let Some(comments_config) = &config.comments {
            if comments_config.min > 0 {
                filters.push(Box::new(CommentFilter {
                    min: comments_config.min,
                }));
            }
        }

        Self {
            mode: config.mode.clone(),
            filters,
        }
    }
}

impl MessageFilter for ChannelFilter {
    fn accept(&self, msg: &Message) -> bool {
        if self.filters.is_empty() {
            return true;
        }
        match self.mode {
            crate::config::FilterMode::Any => self.filters.iter().any(|f| f.accept(msg)),
            crate::config::FilterMode::All => self.filters.iter().all(|f| f.accept(msg)),
        }
    }
}

pub struct ReactionFilter {
    pub specific: Vec<String>,
    pub min_specific_total: i32,
}

pub struct CommentFilter {
    pub min: i32,
}

impl MessageFilter for ReactionFilter {
    fn accept(&self, msg: &Message) -> bool {
        if self.min_specific_total <= 0 && self.specific.is_empty() {
            return true;
        }

        let reactions = match &msg.raw {
            tl::enums::Message::Message(m) => &m.reactions,
            _ => {
                tracing::debug!("ReactionFilter: not a regular message, rejecting");
                return false;
            }
        };

        let Some(tl::enums::MessageReactions::Reactions(r)) = reactions else {
            tracing::debug!("ReactionFilter: no reactions data, rejecting");
            return false;
        };

        let mut total_specific = 0i64;

        for reaction_count in r.results.iter() {
            let tl::enums::ReactionCount::Count(rc) = reaction_count;
            let emoji = reaction_emoji(rc);
            tracing::debug!("ReactionFilter: emoji={}, count={}", emoji, rc.count);
            if self.specific.is_empty() {
                total_specific += rc.count as i64;
            } else {
                if self.specific.contains(&emoji) {
                    total_specific += rc.count as i64;
                }
            }
        }

        let result = total_specific >= self.min_specific_total as i64;
        tracing::debug!(
            "ReactionFilter: total={} min={} specific={:?} result={}",
            total_specific, self.min_specific_total, self.specific, result
        );
        result
    }
}

impl MessageFilter for CommentFilter {
    fn accept(&self, msg: &Message) -> bool {
        if self.min <= 0 {
            return true;
        }

        let replies = match &msg.raw {
            tl::enums::Message::Message(m) => &m.replies,
            _ => return false,
        };

        let Some(tl::enums::MessageReplies::Replies(r)) = replies else {
            return false;
        };

        r.replies >= self.min
    }
}

fn normalize_emoji(s: &str) -> String {
    s.chars().filter(|&c| c != '\u{fe0f}').collect()
}

fn reaction_emoji(r: &tl::types::ReactionCount) -> String {
    let raw = match &r.reaction {
        tl::enums::Reaction::Emoji(e) => e.emoticon.clone(),
        tl::enums::Reaction::CustomEmoji(e) => format!("<custom:{}>", e.document_id),
        tl::enums::Reaction::Empty => String::new(),
        tl::enums::Reaction::Paid => String::new(),
    };
    normalize_emoji(&raw)
}

#[allow(dead_code)]
pub struct AIFilter;

impl MessageFilter for AIFilter {
    fn accept(&self, _msg: &Message) -> bool {
        true
    }
}
