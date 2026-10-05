use std::{io::Cursor, path::Path};

pub struct LoadedFont {
    pub name: String,
    pub data: Vec<u8>,
    pub index: u32,
}

#[derive(Default)]
pub struct AutomaticFonts {
    pub primary: Option<LoadedFont>,
    pub icons: Option<LoadedFont>,
}

pub fn automatic_terminal_fonts() -> AutomaticFonts {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let candidates = home
        .as_ref()
        .map(|home| {
            let home = Path::new(home);
            let config_home = std::env::var_os("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            configured_fonts(home, &config_home, &program)
        })
        .unwrap_or_default();
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let primary = candidates.iter().find_map(|name| {
        let face = db
            .faces()
            .filter(|face| {
                (face.post_script_name.eq_ignore_ascii_case(name)
                    || face
                        .families
                        .iter()
                        .any(|(family, _)| family.eq_ignore_ascii_case(name)))
                    && is_monospaced(&db, face)
            })
            .min_by_key(|face| {
                (
                    !face.post_script_name.eq_ignore_ascii_case(name),
                    face.style != fontdb::Style::Normal,
                    face.weight.0.abs_diff(fontdb::Weight::NORMAL.0),
                    face.post_script_name.clone(),
                )
            })?;
        load_face(&db, face)
    });
    // A Nerd Font supplements private-use prompt icons, without choosing its text style.
    let mut nerd_faces: Vec<_> = db
        .faces()
        .filter(|face| {
            face.style == fontdb::Style::Normal
                && face.weight == fontdb::Weight::NORMAL
                && face.families.iter().any(|(name, _)| is_nerd_family(name))
                && is_monospaced(&db, face)
        })
        .collect();
    nerd_faces.sort_by(|a, b| a.post_script_name.cmp(&b.post_script_name));
    let icons = nerd_faces
        .iter()
        .filter(|face| {
            primary
                .as_ref()
                .is_none_or(|font| font.name != face.post_script_name || font.index != face.index)
        })
        .find_map(|face| load_face(&db, face));
    AutomaticFonts { primary, icons }
}

fn is_monospaced(db: &fontdb::Database, face: &fontdb::FaceInfo) -> bool {
    if face.monospaced {
        return true;
    }
    // Legacy fonts such as Monaco omit the fixed-pitch metadata.
    db.with_face_data(face.id, |data, index| {
        let Ok(font) = ttf_parser::Face::parse(data, index) else {
            return false;
        };
        let advance = |c| {
            font.glyph_index(c)
                .and_then(|glyph| font.glyph_hor_advance(glyph))
        };
        let Some(width) = advance('M').filter(|width| *width > 0) else {
            return false;
        };
        (' '..='~').all(|c| advance(c) == Some(width))
    })
    .unwrap_or(false)
}

fn is_nerd_family(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("nerd font")
        || lower.contains("nerdfont")
        || lower.ends_with(" nf")
        || lower.ends_with(" nfm")
}

fn load_face(db: &fontdb::Database, face: &fontdb::FaceInfo) -> Option<LoadedFont> {
    db.with_face_data(face.id, |data, index| LoadedFont {
        name: face.post_script_name.clone(),
        data: data.to_vec(),
        index,
    })
}

// Prefer the inherited terminal, then iTerm's default, Terminal's default, and kitty.
// Never execute shell/Lua configuration to discover a font.
fn configured_fonts(home: &Path, config_home: &Path, program: &str) -> Vec<String> {
    let iterm = iterm_font(&home.join("Library/Preferences/com.googlecode.iterm2.plist"));
    let terminal = terminal_font(&home.join("Library/Preferences/com.apple.Terminal.plist"));
    let kitty = kitty_font(&config_home.join("kitty/kitty.conf"));
    let first = match program {
        "Apple_Terminal" => terminal.clone(),
        "iTerm.app" => iterm.clone(),
        "kitty" => kitty.clone(),
        _ => None,
    };
    let mut names = Vec::new();
    for name in [first, iterm, terminal, kitty].into_iter().flatten() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn iterm_font(path: &Path) -> Option<String> {
    let value = plist::Value::from_file(path).ok()?;
    let dict = value.as_dictionary()?;
    let default = dict.get("Default Bookmark Guid")?.as_string()?;
    let profile = dict
        .get("New Bookmarks")?
        .as_array()?
        .iter()
        .filter_map(plist::Value::as_dictionary)
        .find(|profile| profile.get("Guid").and_then(plist::Value::as_string) == Some(default))?;
    let font = profile.get("Normal Font")?.as_string()?;
    let (name, size) = font.rsplit_once(' ')?;
    size.parse::<f32>().ok()?;
    Some(name.to_owned())
}

fn terminal_font(path: &Path) -> Option<String> {
    let value = plist::Value::from_file(path).ok()?;
    let dict = value.as_dictionary()?;
    let default = dict.get("Default Window Settings")?.as_string()?;
    let profile = dict
        .get("Window Settings")?
        .as_dictionary()?
        .get(default)?
        .as_dictionary()?;
    let archive = plist::Value::from_reader(Cursor::new(profile.get("Font")?.as_data()?)).ok()?;
    let objects = archive.as_dictionary()?.get("$objects")?.as_array()?;
    let descriptor = objects.get(1)?.as_dictionary()?;
    let name_index = descriptor.get("NSName")?.as_uid()?.get() as usize;
    Some(objects.get(name_index)?.as_string()?.to_owned())
}

fn kitty_font(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (key, value) = line.split_once(char::is_whitespace)?;
            (key == "font_family").then(|| value.trim().to_owned())
        })
        .next_back()
        .filter(|name| !name.is_empty() && name != "auto")
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::{Dictionary, Value};

    #[test]
    fn recognizes_nerd_font_family_conventions() {
        for name in [
            "FiraCode Nerd Font Mono",
            "MesloLGS NF",
            "Symbols NFM",
            "JetBrainsMono NerdFont",
        ] {
            assert!(is_nerd_family(name));
        }
        assert!(!is_nerd_family("FontAwesome"));
        assert!(!is_nerd_family("Monaco"));
    }

    #[test]
    fn verifies_fixed_pitch_when_metadata_is_missing() {
        let fonts = eframe::egui::FontDefinitions::default();
        let monospace = &fonts.families[&eframe::egui::FontFamily::Monospace][0];
        let proportional = &fonts.families[&eframe::egui::FontFamily::Proportional][0];
        let mut db = fontdb::Database::new();
        db.load_font_data(fonts.font_data[monospace].font.to_vec());
        db.load_font_data(fonts.font_data[proportional].font.to_vec());
        let mut fixed = db.faces().find(|face| face.monospaced).unwrap().clone();
        fixed.monospaced = false;
        assert!(is_monospaced(&db, &fixed));
        let variable = db.faces().find(|face| !face.monospaced).unwrap();
        assert!(!is_monospaced(&db, variable));
    }

    #[test]
    fn preferences_are_optional_and_malformed_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.plist");
        assert!(iterm_font(&path).is_none());
        assert!(terminal_font(&path).is_none());
        std::fs::write(&path, "not a plist").unwrap();
        assert!(iterm_font(&path).is_none());
        assert!(terminal_font(&path).is_none());
        assert!(configured_fonts(dir.path(), dir.path(), "unknown").is_empty());
    }

    #[test]
    fn inherited_terminal_takes_precedence_over_iterm_default() {
        let dir = tempfile::tempdir().unwrap();
        let prefs_dir = dir.path().join("Library/Preferences");
        std::fs::create_dir_all(&prefs_dir).unwrap();
        let mut profile = Dictionary::new();
        profile.insert("Guid".into(), Value::String("chosen".into()));
        profile.insert("Normal Font".into(), Value::String("Monaco 12".into()));
        let mut prefs = Dictionary::new();
        prefs.insert(
            "Default Bookmark Guid".into(),
            Value::String("chosen".into()),
        );
        prefs.insert(
            "New Bookmarks".into(),
            Value::Array(vec![Value::Dictionary(profile)]),
        );
        Value::Dictionary(prefs)
            .to_file_binary(prefs_dir.join("com.googlecode.iterm2.plist"))
            .unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir_all(config.join("kitty")).unwrap();
        std::fs::write(config.join("kitty/kitty.conf"), "font_family Kitty Font").unwrap();
        assert_eq!(
            configured_fonts(dir.path(), &config, "kitty"),
            vec!["Kitty Font", "Monaco"]
        );
        assert_eq!(
            configured_fonts(dir.path(), &config, "unknown"),
            vec!["Monaco", "Kitty Font"]
        );
    }

    #[test]
    fn iterm_uses_default_profile_and_preserves_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iterm.plist");
        let mut profile = Dictionary::new();
        profile.insert("Guid".into(), Value::String("chosen".into()));
        profile.insert(
            "Normal Font".into(),
            Value::String("Example Nerd Font Mono 13".into()),
        );
        let mut prefs = Dictionary::new();
        prefs.insert(
            "Default Bookmark Guid".into(),
            Value::String("chosen".into()),
        );
        prefs.insert(
            "New Bookmarks".into(),
            Value::Array(vec![Value::Dictionary(profile)]),
        );
        Value::Dictionary(prefs).to_file_binary(&path).unwrap();
        assert_eq!(iterm_font(&path).as_deref(), Some("Example Nerd Font Mono"));
    }

    #[test]
    fn terminal_reads_font_archive_reference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminal.plist");
        let mut descriptor = Dictionary::new();
        descriptor.insert("NSName".into(), Value::Uid(plist::Uid::new(2)));
        let mut archive = Dictionary::new();
        archive.insert(
            "$objects".into(),
            Value::Array(vec![
                Value::String("$null".into()),
                Value::Dictionary(descriptor),
                Value::String("SFMonoTerminal-Regular".into()),
            ]),
        );
        let mut bytes = Vec::new();
        Value::Dictionary(archive)
            .to_writer_binary(&mut bytes)
            .unwrap();
        let mut profile = Dictionary::new();
        profile.insert("Font".into(), Value::Data(bytes));
        let mut profiles = Dictionary::new();
        profiles.insert("Default".into(), Value::Dictionary(profile));
        let mut prefs = Dictionary::new();
        prefs.insert(
            "Default Window Settings".into(),
            Value::String("Default".into()),
        );
        prefs.insert("Window Settings".into(), Value::Dictionary(profiles));
        Value::Dictionary(prefs).to_file_binary(&path).unwrap();
        assert_eq!(
            terminal_font(&path).as_deref(),
            Some("SFMonoTerminal-Regular")
        );
    }

    #[test]
    fn kitty_uses_last_explicit_family_and_ignores_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kitty.conf");
        std::fs::write(&path, "# font_family Ignored\nfont_family First\nfont_family Example Nerd Font Mono\ninclude other.conf\n").unwrap();
        assert_eq!(kitty_font(&path).as_deref(), Some("Example Nerd Font Mono"));
        std::fs::write(&path, "font_family First\nfont_family auto\n").unwrap();
        assert!(kitty_font(&path).is_none());
    }
}
