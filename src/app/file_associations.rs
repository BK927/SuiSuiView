use super::settings::setting_group;
use super::ui::theme;
use super::{platform, SuiSuiViewApp};
use crate::core::i18n::I18n;
use egui::{self, RichText};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileAssociationCategory {
    Images,
    Comics,
    GenericArchives,
}

impl FileAssociationCategory {
    const ALL: [Self; 3] = [Self::Images, Self::Comics, Self::GenericArchives];

    fn title_key(self) -> &'static str {
        match self {
            Self::Images => "settings.file_associations.images.title",
            Self::Comics => "settings.file_associations.comics.title",
            Self::GenericArchives => "settings.file_associations.archives.title",
        }
    }

    fn description_key(self) -> &'static str {
        match self {
            Self::Images => "settings.file_associations.images.desc",
            Self::Comics => "settings.file_associations.comics.desc",
            Self::GenericArchives => "settings.file_associations.archives.desc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileAssociationTarget {
    id: &'static str,
    label: &'static str,
    extensions: &'static [&'static str],
    category: FileAssociationCategory,
    default_selected: bool,
}

#[derive(Debug, Clone)]
pub(in crate::app) struct FileAssociationSelection {
    selected_target_ids: BTreeSet<&'static str>,
}

impl Default for FileAssociationSelection {
    fn default() -> Self {
        Self {
            selected_target_ids: available_targets()
                .into_iter()
                .filter(|target| target.default_selected)
                .map(|target| target.id)
                .collect(),
        }
    }
}

impl FileAssociationSelection {
    fn retain_available_targets(&mut self, targets: &[FileAssociationTarget]) {
        self.selected_target_ids
            .retain(|id| targets.iter().any(|target| target.id == *id));
    }

    fn is_selected(&self, target: FileAssociationTarget) -> bool {
        self.selected_target_ids.contains(target.id)
    }

    fn set_selected(&mut self, target: FileAssociationTarget, selected: bool) {
        if selected {
            self.selected_target_ids.insert(target.id);
        } else {
            self.selected_target_ids.remove(target.id);
        }
    }

    fn set_category_selected(
        &mut self,
        targets: &[FileAssociationTarget],
        category: FileAssociationCategory,
        selected: bool,
    ) {
        for target in targets
            .iter()
            .copied()
            .filter(|target| target.category == category)
        {
            self.set_selected(target, selected);
        }
    }

    fn category_counts(
        &self,
        targets: &[FileAssociationTarget],
        category: FileAssociationCategory,
    ) -> (usize, usize) {
        let mut selected = 0;
        let mut total = 0;
        for target in targets
            .iter()
            .copied()
            .filter(|target| target.category == category)
        {
            total += 1;
            if self.is_selected(target) {
                selected += 1;
            }
        }
        (selected, total)
    }

    fn selected_extensions(&self, targets: &[FileAssociationTarget]) -> Vec<&'static str> {
        targets
            .iter()
            .copied()
            .filter(|target| self.is_selected(*target))
            .flat_map(|target| target.extensions.iter().copied())
            .collect()
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn show_file_association_settings(&mut self, ui: &mut egui::Ui, i18n: I18n) {
        let targets = available_targets();
        self.file_association_selection
            .retain_available_targets(&targets);

        setting_group(
            ui,
            &i18n.text("settings.file_associations.actions.title"),
            &i18n.text("settings.file_associations.actions.desc"),
            |ui| {
                let selected_extensions = self
                    .file_association_selection
                    .selected_extensions(&targets);
                ui.label(
                    RichText::new(i18n.text("settings.file_associations.note"))
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    let register_label = i18n.text("settings.file_associations.register_and_open");
                    if ui
                        .add_enabled(
                            !selected_extensions.is_empty(),
                            egui::Button::new(register_label),
                        )
                        .on_disabled_hover_text(
                            i18n.text("settings.file_associations.no_selection"),
                        )
                        .clicked()
                    {
                        self.register_selected_file_associations(&targets, i18n);
                    }
                    if ui
                        .button(i18n.text("settings.file_associations.open_default_apps"))
                        .clicked()
                    {
                        self.open_windows_default_apps(i18n);
                    }
                    if ui
                        .button(i18n.text("settings.file_associations.unregister"))
                        .clicked()
                    {
                        self.unregister_file_associations(&targets, i18n);
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(i18n.with_vars(
                        "settings.file_associations.selected_count",
                        &[("count", selected_extensions.len().to_string())],
                    ))
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
                );
            },
        );

        for category in FileAssociationCategory::ALL {
            ui.add_space(8.0);
            setting_group(
                ui,
                &i18n.text(category.title_key()),
                &i18n.text(category.description_key()),
                |ui| {
                    self.show_file_association_category(ui, &targets, category, i18n);
                },
            );
        }
    }

    fn show_file_association_category(
        &mut self,
        ui: &mut egui::Ui,
        targets: &[FileAssociationTarget],
        category: FileAssociationCategory,
        i18n: I18n,
    ) {
        let (selected, total) = self
            .file_association_selection
            .category_counts(targets, category);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(i18n.text("settings.file_associations.select_all"))
                .clicked()
            {
                self.file_association_selection
                    .set_category_selected(targets, category, true);
            }
            if ui
                .button(i18n.text("settings.file_associations.clear"))
                .clicked()
            {
                self.file_association_selection
                    .set_category_selected(targets, category, false);
            }
            ui.label(
                RichText::new(i18n.with_vars(
                    "settings.file_associations.category_count",
                    &[
                        ("selected", selected.to_string()),
                        ("total", total.to_string()),
                    ],
                ))
                .size(12.0)
                .color(theme::TEXT_MUTED),
            );
        });
        ui.add_space(6.0);

        for target in targets
            .iter()
            .copied()
            .filter(|target| target.category == category)
        {
            let mut selected = self.file_association_selection.is_selected(target);
            let extension_text = target.extensions.join(", ");
            ui.horizontal_wrapped(|ui| {
                if ui
                    .checkbox(&mut selected, target.label)
                    .on_hover_text(&extension_text)
                    .changed()
                {
                    self.file_association_selection
                        .set_selected(target, selected);
                }
                ui.label(
                    RichText::new(extension_text)
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
            });
        }
    }

    fn register_selected_file_associations(
        &mut self,
        targets: &[FileAssociationTarget],
        i18n: I18n,
    ) {
        let selected_extensions = self.file_association_selection.selected_extensions(targets);
        if selected_extensions.is_empty() {
            self.notify(i18n.text("settings.file_associations.no_selection"));
            return;
        }
        let known_extensions = all_known_extensions(targets);
        match platform::register_file_associations(&selected_extensions, &known_extensions) {
            Ok(count) => match platform::open_windows_default_apps_for_suisuiview() {
                Ok(()) => self.notify(i18n.with_vars(
                    "status.file_associations.registered",
                    &[("count", count.to_string())],
                )),
                Err(error) => self.notify(i18n.with_vars(
                    "status.file_associations.registered_open_failed",
                    &[("count", count.to_string()), ("error", error)],
                )),
            },
            Err(error) => self.notify(i18n.with_vars(
                "status.file_associations.register_failed",
                &[("error", error)],
            )),
        }
    }

    fn unregister_file_associations(&mut self, targets: &[FileAssociationTarget], i18n: I18n) {
        let known_extensions = all_known_extensions(targets);
        match platform::unregister_file_associations(&known_extensions) {
            Ok(()) => self.notify(i18n.text("status.file_associations.unregistered")),
            Err(error) => self.notify(i18n.with_vars(
                "status.file_associations.unregister_failed",
                &[("error", error)],
            )),
        }
    }

    fn open_windows_default_apps(&mut self, i18n: I18n) {
        match platform::open_windows_default_apps_for_suisuiview() {
            Ok(()) => self.notify(i18n.text("status.file_associations.default_apps_opened")),
            Err(error) => self.notify(i18n.with_vars(
                "status.file_associations.default_apps_open_failed",
                &[("error", error)],
            )),
        }
    }
}

fn available_targets() -> Vec<FileAssociationTarget> {
    let mut targets = BASE_TARGETS.to_vec();
    extend_optional_targets(&mut targets);
    targets
}

fn extend_optional_targets(targets: &mut Vec<FileAssociationTarget>) {
    let _ = targets;
    #[cfg(feature = "native-avif")]
    targets.push(FileAssociationTarget {
        id: "avif",
        label: "AVIF",
        extensions: &[".avif"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    });
    #[cfg(feature = "native-ai")]
    targets.push(FileAssociationTarget {
        id: "ai",
        label: "Adobe Illustrator preview",
        extensions: &[".ai"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    });
}

fn all_known_extensions(targets: &[FileAssociationTarget]) -> Vec<&'static str> {
    targets
        .iter()
        .flat_map(|target| target.extensions.iter().copied())
        .collect()
}

const BASE_TARGETS: &[FileAssociationTarget] = &[
    FileAssociationTarget {
        id: "jpeg",
        label: "JPEG",
        extensions: &[".jpg", ".jpeg", ".jpe", ".jfif"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "png",
        label: "PNG / APNG",
        extensions: &[".png", ".apng"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "webp",
        label: "WebP",
        extensions: &[".webp"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "bmp",
        label: "BMP",
        extensions: &[".bmp", ".dib"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "gif",
        label: "GIF",
        extensions: &[".gif"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "tiff",
        label: "TIFF",
        extensions: &[".tif", ".tiff"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "tga",
        label: "TGA",
        extensions: &[".tga"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "pnm",
        label: "PNM / PBM / PGM / PPM",
        extensions: &[".pnm", ".pbm", ".pgm", ".ppm"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "ico",
        label: "ICO",
        extensions: &[".ico"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "qoi",
        label: "QOI",
        extensions: &[".qoi"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "dds",
        label: "DDS",
        extensions: &[".dds"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "exr",
        label: "OpenEXR",
        extensions: &[".exr"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "hdr",
        label: "Radiance HDR / RGBE",
        extensions: &[".hdr", ".rgbe"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "psd",
        label: "Photoshop PSD preview",
        extensions: &[".psd"],
        category: FileAssociationCategory::Images,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "cbz",
        label: "Comic book ZIP",
        extensions: &[".cbz"],
        category: FileAssociationCategory::Comics,
        default_selected: true,
    },
    FileAssociationTarget {
        id: "zip",
        label: "Generic ZIP archive",
        extensions: &[".zip"],
        category: FileAssociationCategory::GenericArchives,
        default_selected: false,
    },
];

#[cfg(test)]
mod tests {
    use super::{available_targets, FileAssociationCategory, FileAssociationSelection};

    #[test]
    fn default_selection_includes_images_and_cbz_but_not_zip() {
        let targets = available_targets();
        let selection = FileAssociationSelection::default();
        let extensions = selection.selected_extensions(&targets);

        assert!(extensions.contains(&".jpg"));
        assert!(extensions.contains(&".cbz"));
        assert!(!extensions.contains(&".zip"));
    }

    #[test]
    fn generic_archives_can_be_selected_as_a_separate_category() {
        let targets = available_targets();
        let mut selection = FileAssociationSelection::default();
        selection.set_category_selected(&targets, FileAssociationCategory::GenericArchives, true);
        let extensions = selection.selected_extensions(&targets);

        assert!(extensions.contains(&".zip"));
    }
}
