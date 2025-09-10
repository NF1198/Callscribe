#[derive(Clone, Debug)]
pub struct SlotData {
    pub tg: Option<String>,
    pub rid: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RadioRecord {
    pub record_number: usize,
    pub datetime: chrono::DateTime<chrono::FixedOffset>,
    pub frequency: Option<String>,
    pub radio_type: Option<String>,
    pub dcc: Option<String>, // NAC or DCC
    pub slot1: SlotData,
    pub slot2: SlotData,
    pub duration: u32
}