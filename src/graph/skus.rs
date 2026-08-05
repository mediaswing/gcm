//! Translation from SKU part numbers to the product names people recognise.
//!
//! Graph returns `subscribedSku.skuPartNumber` — strings like `SPE_E3`. Nobody
//! administers a tenant thinking in those terms, so the Licenses view shows the
//! marketing name with the part number as a secondary column.
//!
//! Microsoft publishes the authoritative mapping as a CSV that changes monthly;
//! rather than bundle a stale copy, this covers the SKUs that appear in most
//! commercial tenants and degrades to the raw part number for the rest.

/// Common SKU part numbers paired with their current product names.
const KNOWN: &[(&str, &str)] = &[
    // Microsoft 365 enterprise suites
    ("SPE_E3", "Microsoft 365 E3"),
    ("SPE_E5", "Microsoft 365 E5"),
    ("SPE_F1", "Microsoft 365 F3"),
    ("SPE_E3_RPA1", "Microsoft 365 E3 - Unattended License"),
    ("M365_F1_COMM", "Microsoft 365 F1"),
    ("DESKLESSPACK", "Office 365 F3"),
    // Office 365
    ("STANDARDPACK", "Office 365 E1"),
    ("STANDARDWOFFPACK", "Office 365 E2"),
    ("ENTERPRISEPACK", "Office 365 E3"),
    ("ENTERPRISEPREMIUM", "Office 365 E5"),
    ("ENTERPRISEPREMIUM_NOPSTNCONF", "Office 365 E5 without Audio Conferencing"),
    ("ENTERPRISEWITHSCAL", "Office 365 A3"),
    // Microsoft 365 business
    ("O365_BUSINESS_ESSENTIALS", "Microsoft 365 Business Basic"),
    ("O365_BUSINESS_PREMIUM", "Microsoft 365 Apps for Business"),
    ("SMB_BUSINESS_ESSENTIALS", "Microsoft 365 Business Basic"),
    ("SMB_BUSINESS_PREMIUM", "Microsoft 365 Apps for Business"),
    ("SPB", "Microsoft 365 Business Premium"),
    ("SPE_E3_USGOV_DOD", "Microsoft 365 E3 (DoD)"),
    ("SPE_E3_USGOV_GCCHIGH", "Microsoft 365 E3 (GCC High)"),
    // Apps
    ("OFFICESUBSCRIPTION", "Microsoft 365 Apps for Enterprise"),
    ("O365_BUSINESS", "Microsoft 365 Apps for Business"),
    ("VISIOCLIENT", "Visio Plan 2"),
    ("VISIOONLINE_PLAN1", "Visio Plan 1"),
    ("PROJECTPROFESSIONAL", "Project Plan 3"),
    ("PROJECTPREMIUM", "Project Plan 5"),
    ("PROJECTESSENTIALS", "Project Plan 1"),
    // Entra ID
    ("AAD_PREMIUM", "Microsoft Entra ID P1"),
    ("AAD_PREMIUM_P2", "Microsoft Entra ID P2"),
    ("AAD_BASIC", "Microsoft Entra ID Basic"),
    ("Microsoft_Entra_ID_Governance", "Microsoft Entra ID Governance"),
    ("IDENTITY_THREAT_PROTECTION", "Microsoft 365 E5 Security"),
    // Enterprise Mobility + Security
    ("EMS", "Enterprise Mobility + Security E3"),
    ("EMSPREMIUM", "Enterprise Mobility + Security E5"),
    ("INTUNE_A", "Intune Plan 1"),
    ("INTUNE_A_D", "Microsoft Intune Plan 1 Device"),
    ("INTUNE_SMB", "Microsoft Intune for Business"),
    ("Microsoft_Intune_Suite", "Microsoft Intune Suite"),
    ("INTUNE_EDU", "Intune for Education"),
    // Exchange, SharePoint, Teams
    ("EXCHANGESTANDARD", "Exchange Online (Plan 1)"),
    ("EXCHANGEENTERPRISE", "Exchange Online (Plan 2)"),
    ("EXCHANGEDESKLESS", "Exchange Online Kiosk"),
    ("EXCHANGEARCHIVE_ADDON", "Exchange Online Archiving"),
    ("SHAREPOINTSTANDARD", "SharePoint Online (Plan 1)"),
    ("SHAREPOINTENTERPRISE", "SharePoint Online (Plan 2)"),
    ("MCOSTANDARD", "Skype for Business Online (Plan 2)"),
    ("MCOEV", "Microsoft Teams Phone Standard"),
    ("MCOMEETADV", "Microsoft 365 Audio Conferencing"),
    ("MCOPSTN1", "Microsoft 365 Domestic Calling Plan"),
    ("MCOPSTN2", "Microsoft 365 International Calling Plan"),
    ("Teams_Premium_(for_Departments)", "Microsoft Teams Premium"),
    ("TEAMS_EXPLORATORY", "Microsoft Teams Exploratory"),
    ("Microsoft_Teams_Rooms_Pro", "Microsoft Teams Rooms Pro"),
    ("MEETING_ROOM", "Microsoft Teams Rooms Standard"),
    // Security and compliance
    ("ATP_ENTERPRISE", "Microsoft Defender for Office 365 (Plan 1)"),
    ("THREAT_INTELLIGENCE", "Microsoft Defender for Office 365 (Plan 2)"),
    ("ATA", "Microsoft Defender for Identity"),
    ("ADALLOM_STANDALONE", "Microsoft Defender for Cloud Apps"),
    ("DEFENDER_ENDPOINT_P1", "Microsoft Defender for Endpoint P1"),
    ("WIN_DEF_ATP", "Microsoft Defender for Endpoint"),
    ("INFORMATION_PROTECTION_COMPLIANCE", "Microsoft 365 E5 Compliance"),
    ("RIGHTSMANAGEMENT", "Azure Information Protection Plan 1"),
    ("EQUIVIO_ANALYTICS", "Microsoft 365 eDiscovery and Audit"),
    // Power Platform
    ("POWER_BI_STANDARD", "Microsoft Fabric (Free)"),
    ("POWER_BI_PRO", "Power BI Pro"),
    ("PBI_PREMIUM_PER_USER", "Power BI Premium Per User"),
    ("FLOW_FREE", "Microsoft Power Automate Free"),
    ("POWERAPPS_VIRAL", "Microsoft Power Apps Plan 2 Trial"),
    ("POWERAPPS_PER_USER", "Power Apps Premium"),
    ("CDS_DB_CAPACITY", "Common Data Service Database Capacity"),
    // Copilot and AI
    ("Microsoft_365_Copilot", "Microsoft 365 Copilot"),
    ("CPC_E_2C_8GB_128GB", "Windows 365 Enterprise 2 vCPU, 8 GB, 128 GB"),
    ("Microsoft_Copilot_Studio", "Microsoft Copilot Studio"),
    // Windows
    ("WIN10_PRO_ENT_SUB", "Windows 10/11 Enterprise E3"),
    ("WIN10_VDA_E3", "Windows 10/11 Enterprise E3"),
    ("WIN10_VDA_E5", "Windows 10/11 Enterprise E5"),
    ("WINE5_GCC_COMPAT", "Windows 10/11 Enterprise E5 (GCC Compatible)"),
    // Free and trial tiers that clutter most tenants
    ("FLOW_P2_VIRAL", "Flow Free Trial"),
    ("STREAM", "Microsoft Stream"),
    ("MCOFREE", "Microsoft Teams (Free)"),
    ("TEAMS_FREE", "Microsoft Teams (Free)"),
    ("WINDOWS_STORE", "Windows Store for Business"),
    ("SHAREPOINTSTORAGE", "Office 365 Extra File Storage"),
    ("RMSBASIC", "Rights Management Service Basic"),
];

/// The product name for a SKU part number, falling back to the part number.
///
/// Matching is case-insensitive because Graph's casing is not consistent across
/// SKUs (compare `SPE_E3` with `Microsoft_Teams_Rooms_Pro`).
pub fn friendly_name(part_number: &str) -> String {
    KNOWN
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(part_number))
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| part_number.to_string())
}

/// Whether we recognised the part number, so the UI can hint that an unfamiliar
/// SKU is showing its raw identifier rather than a name.
pub fn is_known(part_number: &str) -> bool {
    KNOWN
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case(part_number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_skus() {
        assert_eq!(friendly_name("SPE_E3"), "Microsoft 365 E3");
        assert_eq!(friendly_name("ENTERPRISEPACK"), "Office 365 E3");
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(friendly_name("spe_e5"), "Microsoft 365 E5");
        assert!(is_known("aad_premium_p2"));
    }

    #[test]
    fn falls_back_to_the_part_number() {
        assert_eq!(friendly_name("SOME_NEW_SKU"), "SOME_NEW_SKU");
        assert!(!is_known("SOME_NEW_SKU"));
    }

    #[test]
    fn table_has_no_duplicate_keys() {
        // A duplicate would silently shadow the later entry.
        let mut keys: Vec<String> = KNOWN.iter().map(|(k, _)| k.to_lowercase()).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "duplicate SKU part number in table");
    }
}
