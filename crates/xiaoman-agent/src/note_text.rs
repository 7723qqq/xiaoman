//! Basic Memory Markdown rendering shared by the memory tools and the learning notes.

use xiaoman_api::{Note, Observation, Relation};

pub struct NoteText;

impl NoteText {
    pub fn observation_line(observation: &Observation) -> String {
        let mut out = format!("- [{}] {}", observation.category, observation.content);
        for tag in &observation.tags {
            out.push_str(" #");
            out.push_str(tag);
        }
        if let Some(context) = observation
            .context
            .as_deref()
            .filter(|context| !context.trim().is_empty())
        {
            out.push_str(&format!(" ({context})"));
        }
        out
    }

    pub fn relation_line(relation: &Relation) -> String {
        let mut out = format!("- {} [[{}]]", relation.relation_type, relation.target);
        if let Some(context) = relation
            .context
            .as_deref()
            .filter(|context| !context.trim().is_empty())
        {
            out.push_str(&format!(" ({context})"));
        }
        out
    }

    /// Body with `# title`, any free text of `preamble`, then the Observations and Relations sections.
    /// `preamble` is the part of an existing body before its first section heading.
    pub fn body(
        title: &str,
        observations: &[Observation],
        relations: &[Relation],
        preamble: &str,
    ) -> String {
        let intro = preamble
            .lines()
            .skip_while(|line| line.trim().is_empty() || line.trim() == format!("# {title}"))
            .collect::<Vec<_>>()
            .join("\n");
        let intro = intro.trim();
        let mut out = format!("# {title}\n");
        if !intro.is_empty() {
            out.push('\n');
            out.push_str(intro);
            out.push('\n');
        }
        if !observations.is_empty() {
            out.push_str("\n## Observations\n");
            for observation in observations {
                out.push_str(&Self::observation_line(observation));
                out.push('\n');
            }
        }
        if !relations.is_empty() {
            out.push_str("\n## Relations\n");
            for relation in relations {
                out.push_str(&Self::relation_line(relation));
                out.push('\n');
            }
        }
        out
    }

    /// Free text of a body before `## Observations` / `## Relations`.
    pub fn preamble(body: &str) -> String {
        let mut offset = 0;
        for line in body.split_inclusive('\n') {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed.starts_with("## ") {
                let heading = trimmed.trim_start_matches("## ").trim_end();
                if heading == "Observations" || heading == "Relations" {
                    return body[..offset].to_string();
                }
            }
            offset += line.len();
        }
        body.to_string()
    }

    /// Compact text of a note for prompts, cut to `max_chars`.
    pub fn for_prompt(note: &Note, max_chars: usize) -> String {
        let mut text = format!("### {} ({})\n", note.title, note.permalink);
        if !note.observations.is_empty() || !note.relations.is_empty() {
            for observation in &note.observations {
                text.push_str(&Self::observation_line(observation));
                text.push('\n');
            }
            for relation in &note.relations {
                text.push_str(&Self::relation_line(relation));
                text.push('\n');
            }
            let intro = Self::preamble(&note.body)
                .lines()
                .filter(|line| !line.starts_with("# "))
                .collect::<Vec<_>>()
                .join("\n");
            let intro = intro.trim();
            if !intro.is_empty() {
                text.push_str(intro);
                text.push('\n');
            }
        } else {
            text.push_str(note.body.trim());
            text.push('\n');
        }
        let text = text.trim().to_string();
        if text.chars().count() <= max_chars {
            text
        } else {
            let mut clipped: String = text.chars().take(max_chars).collect();
            clipped.push('…');
            clipped
        }
    }
}
