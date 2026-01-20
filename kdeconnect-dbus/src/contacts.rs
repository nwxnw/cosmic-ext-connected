//! Contact loading and lookup from KDE Connect synced vCard files.
//!
//! KDE Connect syncs contacts as vCard files to ~/.local/share/kpeoplevcard/kdeconnect-{device-id}/

use std::collections::HashMap;
use std::path::PathBuf;

/// A contact with name and phone numbers.
#[derive(Debug, Clone)]
pub struct Contact {
    /// Display name (from FN field).
    pub name: String,
    /// List of phone numbers associated with this contact.
    pub phone_numbers: Vec<String>,
}

/// Contact lookup cache mapping normalized phone numbers to contact names.
#[derive(Debug, Clone, Default)]
pub struct ContactLookup {
    /// Map from full normalized phone number (all digits) to contact name.
    /// Used for exact matching.
    phone_to_name: HashMap<String, String>,
    /// Map from phone suffix (last N digits) to contact name.
    /// Used for fuzzy matching when exact match fails.
    suffix_to_name: HashMap<String, String>,
    /// Full list of contacts for name-based searching.
    contacts: Vec<Contact>,
}

impl ContactLookup {
    /// Create a new empty contact lookup.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load contacts asynchronously from the kpeoplevcard directory for a specific device.
    /// Uses tokio::fs for non-blocking file I/O.
    pub async fn load_for_device(device_id: &str) -> Self {
        let mut lookup = Self::new();

        // Get the kpeoplevcard directory
        let vcard_dir = match dirs::data_local_dir() {
            Some(dir) => dir
                .join("kpeoplevcard")
                .join(format!("kdeconnect-{}", device_id)),
            None => {
                tracing::warn!("Could not find local data directory for contacts");
                return lookup;
            }
        };

        // Check if directory exists using async metadata
        match tokio::fs::metadata(&vcard_dir).await {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                tracing::debug!("vCard path is not a directory: {:?}", vcard_dir);
                return lookup;
            }
            Err(_) => {
                tracing::debug!("vCard directory does not exist: {:?}", vcard_dir);
                return lookup;
            }
        }

        // Read all .vcf files asynchronously
        let mut entries = match tokio::fs::read_dir(&vcard_dir).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Failed to read vCard directory: {}", e);
                return lookup;
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().map(|e| e == "vcf").unwrap_or(false) {
                if let Some(contact) = parse_vcard_async(&path).await {
                    for phone in &contact.phone_numbers {
                        let (full_digits, suffix) = get_phone_keys(phone);
                        if full_digits.len() >= MIN_PHONE_DIGITS {
                            // Store in exact match map
                            lookup
                                .phone_to_name
                                .insert(full_digits, contact.name.clone());
                            // Store in suffix map for fuzzy matching
                            // (only if suffix differs from full, to avoid duplicates)
                            if suffix.len() >= MIN_PHONE_DIGITS {
                                lookup.suffix_to_name.insert(suffix, contact.name.clone());
                            }
                        }
                    }
                    lookup.contacts.push(contact);
                }
            }
        }

        // Sort contacts alphabetically by name for consistent display
        lookup
            .contacts
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        tracing::info!(
            "Loaded {} contacts with {} exact + {} suffix phone mappings",
            lookup.contacts.len(),
            lookup.phone_to_name.len(),
            lookup.suffix_to_name.len()
        );
        lookup
    }

    /// Look up a contact name by phone number.
    /// Tries exact match first, then falls back to suffix matching.
    /// Returns None if no contact is found.
    pub fn get_name(&self, phone_number: &str) -> Option<&str> {
        let (full_digits, suffix) = get_phone_keys(phone_number);

        // Skip lookup for numbers that are too short
        if full_digits.len() < MIN_PHONE_DIGITS {
            return None;
        }

        // Try exact match first
        if let Some(name) = self.phone_to_name.get(&full_digits) {
            return Some(name.as_str());
        }

        // Fall back to suffix matching
        if suffix.len() >= MIN_PHONE_DIGITS {
            if let Some(name) = self.suffix_to_name.get(&suffix) {
                return Some(name.as_str());
            }
        }

        None
    }

    /// Look up a contact name by phone number, returning the phone number if not found.
    /// Also falls back to phone number if the stored name is empty.
    pub fn get_name_or_number(&self, phone_number: &str) -> String {
        self.get_name(phone_number)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| phone_number.to_string())
    }

    /// Returns the number of contacts loaded.
    pub fn len(&self) -> usize {
        self.phone_to_name.len()
    }

    /// Returns true if no contacts are loaded.
    pub fn is_empty(&self) -> bool {
        self.phone_to_name.is_empty()
    }

    /// Search contacts by name (case-insensitive prefix/substring match).
    /// Returns up to `limit` matching contacts.
    pub fn search_by_name(&self, query: &str, limit: usize) -> Vec<&Contact> {
        if query.is_empty() {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();

        self.contacts
            .iter()
            .filter(|c| c.name.to_lowercase().contains(&query_lower))
            .take(limit)
            .collect()
    }

    /// Get all contacts (for browsing).
    pub fn all_contacts(&self) -> &[Contact] {
        &self.contacts
    }
}

/// Minimum number of digits for a valid phone number match.
const MIN_PHONE_DIGITS: usize = 7;

/// Number of trailing digits to use for suffix matching.
/// This handles country code variations across different regions.
const SUFFIX_MATCH_DIGITS: usize = 10;

/// Normalize a phone number by removing non-digit characters.
/// Returns the full digit string for exact matching.
fn normalize_phone_number(phone: &str) -> String {
    phone.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Get the suffix of a phone number for fuzzy matching.
/// Returns the last SUFFIX_MATCH_DIGITS digits, or all digits if shorter.
/// This handles country code variations (e.g., +1-555-123-4567 matches 5551234567).
fn phone_suffix(digits: &str) -> &str {
    if digits.len() > SUFFIX_MATCH_DIGITS {
        &digits[digits.len() - SUFFIX_MATCH_DIGITS..]
    } else {
        digits
    }
}

/// Get all normalized forms of a phone number for storage/lookup.
/// Returns (full_digits, suffix) where suffix may equal full_digits for short numbers.
fn get_phone_keys(phone: &str) -> (String, String) {
    let digits = normalize_phone_number(phone);
    let suffix = phone_suffix(&digits).to_string();
    (digits, suffix)
}

/// Parse a vCard file asynchronously and extract the contact information.
async fn parse_vcard_async(path: &PathBuf) -> Option<Contact> {
    let content = tokio::fs::read_to_string(path).await.ok()?;

    let mut name = String::new();
    let mut phone_numbers = Vec::new();

    for line in content.lines() {
        // Handle FN (Full Name) field
        if let Some(fn_value) = line.strip_prefix("FN:") {
            name = fn_value.trim().to_string();
        }
        // Handle TEL (telephone) fields - various formats
        else if line.starts_with("TEL") {
            // TEL;CELL:1234567890
            // TEL;TYPE=CELL:1234567890
            // TEL:1234567890
            if let Some(idx) = line.find(':') {
                let number = line[idx + 1..].trim().to_string();
                if !number.is_empty() && !number.contains('=') {
                    // Skip encoded numbers for now
                    phone_numbers.push(number);
                }
            }
        }
    }

    if name.is_empty() || phone_numbers.is_empty() {
        return None;
    }

    Some(Contact {
        name,
        phone_numbers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_phone_number() {
        // Basic normalization - just extract digits
        assert_eq!(normalize_phone_number("(555) 123-4567"), "5551234567");
        assert_eq!(normalize_phone_number("+1-555-123-4567"), "15551234567");
        assert_eq!(normalize_phone_number("15551234567"), "15551234567");
        assert_eq!(normalize_phone_number("555.123.4567"), "5551234567");
    }

    #[test]
    fn test_phone_suffix() {
        // Numbers <= 10 digits return as-is
        assert_eq!(phone_suffix("5551234567"), "5551234567");
        assert_eq!(phone_suffix("1234567"), "1234567");

        // Numbers > 10 digits return last 10
        assert_eq!(phone_suffix("15551234567"), "5551234567");
        assert_eq!(phone_suffix("4915551234567"), "5551234567"); // German country code
    }

    #[test]
    fn test_get_phone_keys() {
        // US number with country code
        let (full, suffix) = get_phone_keys("+1-555-123-4567");
        assert_eq!(full, "15551234567");
        assert_eq!(suffix, "5551234567");

        // US number without country code
        let (full, suffix) = get_phone_keys("(555) 123-4567");
        assert_eq!(full, "5551234567");
        assert_eq!(suffix, "5551234567"); // Same since <= 10 digits

        // International number
        let (full, suffix) = get_phone_keys("+49-555-123-4567");
        assert_eq!(full, "495551234567");
        assert_eq!(suffix, "5551234567");
    }

    #[test]
    fn test_suffix_matching() {
        let mut lookup = ContactLookup::new();

        // Simulate contact stored with country code
        let contact_phone = "15551234567"; // US format with 1
        lookup
            .phone_to_name
            .insert(contact_phone.to_string(), "John Doe".to_string());
        lookup
            .suffix_to_name
            .insert("5551234567".to_string(), "John Doe".to_string());

        // Should match with country code
        assert_eq!(lookup.get_name("+1-555-123-4567"), Some("John Doe"));

        // Should match without country code via suffix
        assert_eq!(lookup.get_name("555-123-4567"), Some("John Doe"));

        // Should match formatted differently
        assert_eq!(lookup.get_name("(555) 123-4567"), Some("John Doe"));
    }
}
