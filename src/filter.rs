use crate::model::RadioRecord;

#[derive(Clone, Debug)]
pub struct FilterConfig {
    pub freqs: Vec<String>,
    pub rtypes: Vec<String>,
    pub rids: Vec<String>,
    pub tgs: Vec<String>,
    pub nacs: Vec<String>,
}

impl FilterConfig {
    pub fn accept(&self, r: &RadioRecord) -> bool {
        if !self.freqs.is_empty() {
            match &r.frequency {
                Some(f) if self.freqs.iter().any(|q| q == f) => {}
                _ => return false,
            }
        }
        if !self.rtypes.is_empty() {
            match &r.radio_type {
                Some(t) if self.rtypes.iter().any(|q| q == t) => {}
                _ => return false,
            }
        }
        if !self.rids.is_empty() {
            match &r.slot1.rid {
                Some(id) if self.rids.iter().any(|q| q == id) => {}
                _ => return false,
            }
        }
        if !self.tgs.is_empty() {
            match &r.slot1.tg {
                Some(tg) if self.tgs.iter().any(|q| q == tg) => {}
                _ => return false,
            }
        }
        if !self.nacs.is_empty() {
            match &r.dcc {
                Some(d) if self.nacs.iter().any(|q| q == d) => {}
                _ => return false,
            }
        }
        true
    }
}
