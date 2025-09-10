use chrono::{DateTime, FixedOffset};

#[derive(Debug, Clone)]
pub struct SlotData {
    pub tg: Option<String>,
    pub rid: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RadioRecord {
    pub record_number: usize,
    pub datetime: DateTime<FixedOffset>,
    pub frequency: Option<String>,
    pub radio_type: Option<String>,
    pub dcc: Option<String>,
    pub slot1: SlotData,
    pub slot2: SlotData
}

