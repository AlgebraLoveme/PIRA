use crate::util;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const MAGIC: &[u8; 8] = b"PIRADEC1";
pub const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_CONTEXT_BYTES: usize = 16 * 1024;
const MAX_CHOICES: usize = 32;
const MAX_CHOICE_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Maker {
    Human,
    Agent,
}

impl Maker {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "human" => Ok(Self::Human),
            "agent" => Ok(Self::Agent),
            _ => Err(format!("invalid maker {value:?}; expected human or agent")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    fn byte(self) -> u8 {
        match self {
            Self::Human => 1,
            Self::Agent => 2,
        }
    }

    fn from_byte(value: u8) -> Result<Self, String> {
        match value {
            1 => Ok(Self::Human),
            2 => Ok(Self::Agent),
            _ => Err("unknown maker value".into()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecisionDraft {
    pub context: String,
    pub choices: Vec<String>,
    pub decision: u32,
    pub makers: Vec<Maker>,
}

impl DecisionDraft {
    pub fn normalized(mut self) -> Result<Self, String> {
        self.context = self.context.trim().to_string();
        self.choices = self
            .choices
            .into_iter()
            .map(|choice| choice.trim().to_string())
            .collect();
        self.makers.sort_unstable();
        validate_fields(&self.context, &self.choices, self.decision, &self.makers)?;
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionRecord {
    pub id: String,
    pub timestamp_ms: u64,
    pub context: String,
    pub choices: Vec<String>,
    pub decision: u32,
    pub makers: Vec<Maker>,
}

impl DecisionRecord {
    pub fn from_draft(
        id: String,
        timestamp_ms: u64,
        draft: &DecisionDraft,
    ) -> Result<Self, String> {
        let record = Self {
            id,
            timestamp_ms,
            context: draft.context.clone(),
            choices: draft.choices.clone(),
            decision: draft.decision,
            makers: draft.makers.clone(),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn selected_text(&self) -> Result<&str, String> {
        let index = usize::try_from(self.decision)
            .ok()
            .and_then(|value| value.checked_sub(1))
            .ok_or_else(|| "decision index is out of range".to_string())?;
        self.choices
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| "decision index is out of range".to_string())
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_id(&self.id, self.timestamp_ms)?;
        util::validate_timestamp(self.timestamp_ms)?;
        validate_fields(&self.context, &self.choices, self.decision, &self.makers)?;
        if self.context.trim() != self.context {
            return Err("context is not stored in normalized form".into());
        }
        if self.choices.iter().any(|choice| choice.trim() != choice) {
            return Err("choice is not stored in normalized form".into());
        }
        if !self.makers.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("makers are not stored as a unique ordered set".into());
        }
        Ok(())
    }

    pub fn view(&self) -> Result<DecisionView, String> {
        Ok(DecisionView {
            id: self.id.clone(),
            timestamp_ms: self.timestamp_ms,
            timestamp: util::format_rfc3339(self.timestamp_ms)?,
            context: self.context.clone(),
            choices: self.choices.clone(),
            decision: self.decision,
            decision_text: self.selected_text()?.to_string(),
            makers: self
                .makers
                .iter()
                .map(|maker| maker.as_str().to_string())
                .collect(),
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DecisionView {
    pub id: String,
    pub timestamp_ms: u64,
    pub timestamp: String,
    pub context: String,
    pub choices: Vec<String>,
    pub decision: u32,
    pub decision_text: String,
    pub makers: Vec<String>,
}

fn validate_fields(
    context: &str,
    choices: &[String],
    decision: u32,
    makers: &[Maker],
) -> Result<(), String> {
    if context.is_empty() {
        return Err("context must not be empty".into());
    }
    if context.len() > MAX_CONTEXT_BYTES {
        return Err(format!("context exceeds {MAX_CONTEXT_BYTES} UTF-8 bytes"));
    }
    if !(2..=MAX_CHOICES).contains(&choices.len()) {
        return Err(format!("expected 2 through {MAX_CHOICES} choices"));
    }
    let mut unique = HashSet::with_capacity(choices.len());
    for choice in choices {
        if choice.is_empty() {
            return Err("choices must not be empty".into());
        }
        if choice.len() > MAX_CHOICE_BYTES {
            return Err(format!("choice exceeds {MAX_CHOICE_BYTES} UTF-8 bytes"));
        }
        if !unique.insert(choice) {
            return Err("choices must be unique after trimming".into());
        }
    }
    if decision == 0 || usize::try_from(decision).map_or(true, |value| value > choices.len()) {
        return Err("decision must select an existing one-based choice".into());
    }
    if makers.is_empty() || makers.len() > 2 {
        return Err("makers must contain human, agent, or both".into());
    }
    if !makers.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err("makers must be unique".into());
    }
    Ok(())
}

pub fn validate_id(id: &str, timestamp_ms: u64) -> Result<(), String> {
    validate_id_syntax(id)?;
    let expected = format!("D-{}-", util::format_id_timestamp(timestamp_ms)?);
    if !id.starts_with(&expected) {
        return Err("decision ID timestamp does not match timestamp_ms".into());
    }
    Ok(())
}

pub fn validate_id_syntax(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    if bytes.len() > MAX_ID_BYTES
        || bytes.len() != 34
        || !id.starts_with("D-")
        || bytes[10] != b'-'
        || bytes[17] != b'-'
        || !bytes[2..10].iter().all(u8::is_ascii_digit)
        || !bytes[11..17].iter().all(u8::is_ascii_digit)
        || !bytes[18..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err("invalid decision ID".into());
    }
    Ok(())
}

pub fn encode(record: &DecisionRecord) -> Result<Vec<u8>, String> {
    record.validate()?;
    let mut body = Vec::new();
    put_tlv(&mut body, 1, record.id.as_bytes())?;
    put_tlv(&mut body, 2, &record.timestamp_ms.to_le_bytes())?;
    put_tlv(&mut body, 3, record.context.as_bytes())?;
    for choice in &record.choices {
        put_tlv(&mut body, 4, choice.as_bytes())?;
    }
    put_tlv(&mut body, 5, &record.decision.to_le_bytes())?;
    for maker in &record.makers {
        put_tlv(&mut body, 6, &[maker.byte()])?;
    }
    let body_len = u32::try_from(body.len()).map_err(|_| "decision body is too large")?;
    let total = 8 + 4 + body.len() + 32;
    if total > MAX_RECORD_BYTES {
        return Err(format!("encoded decision exceeds {MAX_RECORD_BYTES} bytes"));
    }
    let digest = Sha256::digest(&body);
    let mut output = Vec::with_capacity(total);
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&body_len.to_le_bytes());
    output.extend_from_slice(&body);
    output.extend_from_slice(&digest);
    Ok(output)
}

pub fn decode(bytes: &[u8]) -> Result<DecisionRecord, String> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("decision record exceeds size limit".into());
    }
    if bytes.len() < 8 + 4 + 32 || &bytes[..8] != MAGIC {
        return Err("invalid decision record header".into());
    }
    let body_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let expected = 8_usize
        .checked_add(4)
        .and_then(|value| value.checked_add(body_len))
        .and_then(|value| value.checked_add(32))
        .ok_or_else(|| "decision record length overflow".to_string())?;
    if bytes.len() != expected {
        return Err("decision record length mismatch".into());
    }
    let body = &bytes[12..12 + body_len];
    let digest = Sha256::digest(body);
    if digest.as_slice() != &bytes[12 + body_len..] {
        return Err("decision record checksum mismatch".into());
    }

    let mut id = None;
    let mut timestamp_ms = None;
    let mut context = None;
    let mut choices = Vec::new();
    let mut decision = None;
    let mut makers = Vec::new();
    let mut position = 0;
    let mut previous_tag = 0;
    while position < body.len() {
        if body.len() - position < 5 {
            return Err("truncated decision TLV".into());
        }
        let tag = body[position];
        position += 1;
        let length = u32::from_le_bytes(body[position..position + 4].try_into().unwrap()) as usize;
        position += 4;
        let end = position
            .checked_add(length)
            .filter(|end| *end <= body.len())
            .ok_or_else(|| "invalid decision TLV length".to_string())?;
        let value = &body[position..end];
        position = end;
        if tag < previous_tag {
            return Err("decision TLV tags are out of order".into());
        }
        previous_tag = tag;
        match tag {
            1 if id.is_none() => id = Some(parse_string(value, MAX_ID_BYTES, "id")?),
            2 if timestamp_ms.is_none() && value.len() == 8 => {
                timestamp_ms = Some(u64::from_le_bytes(value.try_into().unwrap()));
            }
            3 if context.is_none() => {
                context = Some(parse_string(value, MAX_CONTEXT_BYTES, "context")?);
            }
            4 => choices.push(parse_string(value, MAX_CHOICE_BYTES, "choice")?),
            5 if decision.is_none() && value.len() == 4 => {
                decision = Some(u32::from_le_bytes(value.try_into().unwrap()));
            }
            6 if value.len() == 1 => makers.push(Maker::from_byte(value[0])?),
            1..=6 => return Err("duplicate or malformed singleton decision field".into()),
            _ => return Err("unknown decision TLV tag".into()),
        }
    }
    let record = DecisionRecord {
        id: id.ok_or_else(|| "missing decision id".to_string())?,
        timestamp_ms: timestamp_ms.ok_or_else(|| "missing decision timestamp".to_string())?,
        context: context.ok_or_else(|| "missing decision context".to_string())?,
        choices,
        decision: decision.ok_or_else(|| "missing selected decision".to_string())?,
        makers,
    };
    record.validate()?;
    Ok(record)
}

fn put_tlv(body: &mut Vec<u8>, tag: u8, value: &[u8]) -> Result<(), String> {
    let length = u32::try_from(value.len()).map_err(|_| "decision field is too large")?;
    body.push(tag);
    body.extend_from_slice(&length.to_le_bytes());
    body.extend_from_slice(value);
    Ok(())
}

fn parse_string(value: &[u8], maximum: usize, label: &str) -> Result<String, String> {
    if value.len() > maximum {
        return Err(format!("{label} exceeds size limit"));
    }
    std::str::from_utf8(value)
        .map(str::to_string)
        .map_err(|_| format!("{label} is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DecisionRecord {
        DecisionRecord {
            id: "D-20260717-063012-a3f921c84d77e102".into(),
            timestamp_ms: 1_784_269_812_000,
            context: "Choose storage.".into(),
            choices: vec!["Use SQL.".into(), "Use checked files.".into()],
            decision: 2,
            makers: vec![Maker::Human],
        }
    }

    #[test]
    fn record_round_trips() {
        let record = sample();
        assert_eq!(decode(&encode(&record).unwrap()).unwrap(), record);
    }

    #[test]
    fn checksum_corruption_is_rejected() {
        let mut bytes = encode(&sample()).unwrap();
        bytes[20] ^= 1;
        assert!(decode(&bytes).unwrap_err().contains("checksum"));
    }

    #[test]
    fn invalid_decision_index_is_rejected() {
        let mut record = sample();
        record.decision = 3;
        assert!(record.validate().unwrap_err().contains("one-based"));
    }

    #[test]
    fn decision_view_resolves_selected_text() {
        let view = sample().view().unwrap();
        assert_eq!(view.decision, 2);
        assert_eq!(view.decision_text, "Use checked files.");
    }
}
