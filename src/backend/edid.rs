// SPDX-License-Identifier: MIT OR Apache-2.0
//! Identité d'un écran lue dans son EDID (fabricant, modèle, numéro de
//! série), annoncée aux outils d'écrans (wlr-randr, kanshi, wdisplays) comme
//! le fait wlroots. Fabricant: nom de `/usr/share/hwdata/pnp.ids`, sinon son
//! code PNP à trois lettres.

use std::path::Path;

/// Identité d'un écran
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub make: String,
    pub model: String,
    pub serial: String,
}

/// Numéro de série d'un écran, gardé dans les données de son `Output`
#[derive(Debug, Clone)]
pub struct Serial(pub String);

/// EDID du connecteur `connector` (`DP-2`) du GPU de numéro `dev`, par sysfs
pub fn read(dev: libc::dev_t, connector: &str) -> Option<Identity> {
    let card = Path::new(&format!(
        "/sys/dev/char/{}:{}",
        libc::major(dev),
        libc::minor(dev)
    ))
    .canonicalize()
    .ok()?;
    let name = card.file_name()?.to_str()?.to_owned();
    let data = std::fs::read(card.join(format!("{name}-{connector}")).join("edid")).ok()?;
    let mut id = parse(&data)?;
    if let Some(full) = pnp_name(&id.make) {
        id.make = full;
    }
    Some(id)
}

/// Fabricant, modèle et numéro de série d'un bloc EDID de base (128 octets)
pub fn parse(data: &[u8]) -> Option<Identity> {
    if data.len() < 128 || data[..8] != [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0] {
        return None;
    }
    // Code PNP: trois lettres de 5 bits, 1 = 'A'
    let code = u16::from_be_bytes([data[8], data[9]]);
    let make: String = [10, 5, 0]
        .iter()
        .map(|s| (((code >> s) & 0x1f) as u8).wrapping_add(b'A' - 1) as char)
        .collect();
    let product = u16::from_le_bytes([data[10], data[11]]);
    let serial_num = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    let mut id = Identity {
        make,
        ..Identity::default()
    };
    // Descripteurs d'affichage: 0xfc nom, 0xff numéro de série
    for d in data[54..126].chunks(18) {
        if d[..3] != [0, 0, 0] {
            continue;
        }
        let text = descriptor_text(&d[5..18]);
        match d[3] {
            0xfc => id.model = text,
            0xff => id.serial = text,
            _ => {}
        }
    }
    if id.model.is_empty() {
        id.model = format!("0x{product:04X}");
    }
    if id.serial.is_empty() && serial_num != 0 && serial_num != 0x0101_0101 {
        id.serial = format!("0x{serial_num:08X}");
    }
    Some(id)
}

/// Texte d'un descripteur: fin au saut de ligne, espaces de remplissage ôtés
fn descriptor_text(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == b'\n').unwrap_or(raw.len());
    raw[..end]
        .iter()
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .map(|&b| b as char)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Nom du fabricant de code PNP `code` (hwdata)
fn pnp_name(code: &str) -> Option<String> {
    let ids = std::fs::read_to_string("/usr/share/hwdata/pnp.ids").ok()?;
    ids.lines()
        .find_map(|l| l.strip_prefix(code)?.strip_prefix('\t'))
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    // ViewSonic VA3209-FHD (DP)
    const VIEWSONIC: &str = "00 ff ff ff ff ff ff 00 5a 63 3d a7 58 4f 00 00
        05 22 01 03 80 46 27 78 2e ce 65 a6 57 51 9f 27 0f 50 54 bf ef 80 b3 00 a9 40 a9 c0 95 00 90 40
        81 80 81 40 81 c0 1a 36 80 a0 70 38 1f 40 30 20 35 00 ba 89 21 00 00 1a 00 00 00 ff 00 57 59 4b
        32 34 30 35 32 30 33 31 32 0a 00 00 00 fd 00 30 4b 18 56 19 00 0a 20 20 20 20 20 20 00 00 00 fc
        00 56 41 33 32 30 39 2d 46 48 44 0a 20 20 01 b6";
    // Iiyama PL2773H (HDMI), numéro de série vide
    const IIYAMA: &str = "00 ff ff ff ff ff ff 00 26 cd 0a 66 01 01 01 01
        00 16 01 03 80 3c 22 78 2a b2 05 a3 56 4f 9e 28 0f 50 54 bf ef 80 b3 00 a9 40 95 00 81 40 81 80
        95 0f 71 4f d1 c0 02 3a 80 18 71 38 2d 40 58 2c 45 00 56 50 21 00 00 1e 00 00 00 ff 00 30 0a 20
        20 20 20 20 20 20 20 20 20 20 00 00 00 fd 00 37 4c 1e 53 11 00 0a 20 20 20 20 20 20 00 00 00 fc
        00 50 4c 32 37 37 33 48 0a 20 20 20 20 20 01 1c";

    #[test]
    fn viewsonic() {
        let id = parse(&hex(VIEWSONIC)).unwrap();
        assert_eq!(id.make, "VSC");
        assert_eq!(id.model, "VA3209-FHD");
        assert_eq!(id.serial, "WYK240520312");
    }

    #[test]
    fn iiyama() {
        let id = parse(&hex(IIYAMA)).unwrap();
        assert_eq!(id.make, "IVM");
        assert_eq!(id.model, "PL2773H");
        assert_eq!(id.serial, "0");
    }

    #[test]
    fn pas_un_edid() {
        assert_eq!(parse(&[0; 128]), None);
        assert_eq!(parse(&hex(VIEWSONIC)[..100]), None);
    }
}
