//! A project's icon editor (the ✎ on a row of the project picker): its mark is automatic (its
//! own icon, else its initials), an image file, or letters of your choosing in a colour.
//! Changes apply and save as they're made; Done or Escape closes it.

use std::path::PathBuf;

use rune::{TextEdit, Tx};

use crate::app::App;
use crate::project_icon::{Choice, IMAGE_EXTENSIONS, letters};

pub struct IconEdit {
    /// The project's folder.
    pub path: PathBuf,
    /// The letters field.
    pub letters: TextEdit,
    /// Why the last image couldn't be used.
    pub error: Option<String>,
}

impl App {
    pub fn open_icon_editor(&mut self, path: PathBuf) {
        let Some(p) = self.projects.iter().find(|p| p.path == path) else { return };
        let typed = match &p.choice { Choice::Letters { text, .. } => text.clone(), _ => String::new() };
        self.close_project_picker();
        self.draft.focused = false;
        self.icon_edit = Some(IconEdit { path, letters: TextEdit::new(&typed), error: None });
    }

    pub fn close_icon_editor(&mut self) {
        self.icon_edit = None;
        self.draft.focused = true;
    }

    /// Gives the edited project `choice`, looks for its image again and saves it.
    pub fn set_icon_choice(&mut self, choice: Choice) {
        let Some(path) = self.icon_edit.as_ref().map(|e| e.path.clone()) else { return };
        let Some(p) = self.projects.iter_mut().find(|p| p.path == path) else { return };
        if p.choice == choice { return; }
        p.choice = choice;
        p.icon_looked = false;
        p.icon = None;
        self.find_project_icons();
        self.save_icon_choices();
    }

    /// The editor's Automatic / Image / Letters control. Image asks for a file first if the
    /// project has none chosen yet.
    pub fn pick_icon_mode(&mut self, mode: usize, tx: &mut Tx) {
        let Some(edit) = &self.icon_edit else { return };
        let current = self.projects.iter().find(|p| p.path == edit.path).map(|p| (p.choice.clone(), p.tint));
        let Some((choice, tint)) = current else { return };
        match mode {
            0 => self.set_icon_choice(Choice::Auto),
            1 => match choice {
                Choice::Image(_) => {}
                _ => self.choose_icon_image(tx),
            },
            _ => {
                if !matches!(choice, Choice::Letters { .. }) {
                    let text = letters(&edit.letters.text);
                    self.set_icon_choice(Choice::Letters { text, tint: Some(tint) });
                }
            }
        }
    }

    /// Asks for an image file for the edited project's mark.
    pub fn choose_icon_image(&mut self, tx: &mut Tx) {
        let Some(edit) = &self.icon_edit else { return };
        let directory = Some(edit.path.clone());
        let options = rune::FileDialogOptions { title: "Choose an Icon".into(), directory, ..Default::default() }
            .filter("Images", &IMAGE_EXTENSIONS);
        tx.open_file(options, |s: &mut App, result, _: &mut Tx| match result {
            Ok(Some(file)) => {
                let ok = file.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
                if let Some(edit) = &mut s.icon_edit {
                    edit.error = (!ok).then(|| "That isn't an SVG, PNG, JPEG or ICNS image.".to_owned());
                }
                if ok { s.set_icon_choice(Choice::Image(file)); }
            }
            Ok(None) => {}
            Err(e) => if let Some(edit) = &mut s.icon_edit { edit.error = Some(format!("Couldn't open the file: {e}")); },
        });
    }

    /// The letters field changed: up to two letters, upper case; empty means the initials.
    pub fn icon_letters_typed(&mut self) {
        let Some(edit) = &self.icon_edit else { return };
        let text = letters(&edit.letters.text);
        let tint = self.projects.iter().find(|p| p.path == edit.path).map(|p| p.choice.tint().unwrap_or(p.tint));
        self.set_icon_choice(Choice::Letters { text, tint });
    }

    /// A colour swatch: letters in tint `i` (keeping the letters already typed).
    pub fn pick_icon_tint(&mut self, i: usize) {
        let Some(edit) = &self.icon_edit else { return };
        let text = letters(&edit.letters.text);
        self.set_icon_choice(Choice::Letters { text, tint: Some(i) });
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::app::{App, Project};
    use crate::project_icon::{Choice, load_choices};

    #[test]
    fn letters_and_tints_save_and_come_back() {
        let dir = std::env::temp_dir().join(format!("yoke-icon-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = App::new(Some(dir.clone()), rune::prefs::Prefs::memory());
        s.projects = vec![Project::new(dir.clone())];
        s.find_project_icons();
        s.open_icon_editor(dir.clone());
        assert!(s.icon_edit.is_some());
        s.pick_icon_tint(4);
        s.icon_edit.as_mut().unwrap().letters = rune::TextEdit::new("qz9");
        s.icon_letters_typed();
        assert_eq!(s.projects[0].choice, Choice::Letters { text: "QZ".into(), tint: Some(4) });
        assert_eq!(s.projects[0].tint, 4);
        let saved = load_choices(s.prefs.get(crate::app::PROJECT_ICONS_KEY));
        assert_eq!(saved.get(&dir), Some(&s.projects[0].choice));

        // Back to automatic: nothing saved for it, and its tint is its own again.
        s.set_icon_choice(Choice::Auto);
        assert!(load_choices(s.prefs.get(crate::app::PROJECT_ICONS_KEY)).get(&dir).is_none());
        s.close_icon_editor();
        assert!(s.icon_edit.is_none());

        // A project added later takes the choice saved for its folder.
        s.icon_edit = Some(crate::icon_editor::IconEdit { path: dir.clone(), letters: rune::TextEdit::new(""), error: None });
        s.set_icon_choice(Choice::Image(PathBuf::from("/nope.png")));
        s.projects = vec![Project::new(dir.clone())];
        s.find_project_icons();
        assert_eq!(s.projects[0].choice, Choice::Image(PathBuf::from("/nope.png")));
        assert_eq!(s.projects[0].icon_file, None, "a missing file falls back to letters");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
