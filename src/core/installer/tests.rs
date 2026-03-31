use super::archive::find_common_prefix;
use super::extract::{extract_zip_to, normalize_paths_to_lowercase};
use super::fomod::{decode_fomod_xml, parse_fomod_xml};
use super::heuristics::{
    archive_has_data_folder, build_data_archive_plan, detect_data_root, find_data_root_in_paths,
    find_fomod_parent_dir, score_as_data_root, score_as_data_root_owned,
};
use super::install::install_fomod_files;
use super::paths::{is_safe_relative_path, normalize_path_lowercase, strip_data_prefix};
use super::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

#[test]
fn list_archive_entries_with_7z_returns_paths() {
    let tmp = tempdir();
    let Some(archive) = create_test_7z(&tmp, "list_test.7z") else {
        return;
    };
    let entries = list_archive_entries_with_7z(&archive).unwrap();
    let lower: Vec<String> = entries
        .iter()
        .map(|p| p.to_lowercase().replace('\\', "/"))
        .collect();
    assert!(
        lower.iter().any(|p| p.ends_with("fomod/moduleconfig.xml")),
        "expected fomod/moduleconfig.xml in listing, got: {lower:?}"
    );
    assert!(
        lower.iter().any(|p| p.ends_with("data/textures/sky.dds")),
        "expected Data/textures/sky.dds in listing, got: {lower:?}"
    );
}

/// Create a simple zip archive in `dir` containing the given entries.
fn create_test_zip(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let archive_path = dir.join("test_mod.zip");
    let file = std::fs::File::create(&archive_path).unwrap();
    let mut zip_writer = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for &(name, content) in entries {
        if name.ends_with('/') {
            zip_writer.add_directory(name, options).unwrap();
        } else {
            zip_writer.start_file(name, options).unwrap();
            zip_writer.write_all(content).unwrap();
        }
    }
    let inner = zip_writer.finish().unwrap();
    drop(inner);
    archive_path
}

fn create_test_7z(root: &Path, archive_name: &str) -> Option<PathBuf> {
    let archive_path = root.join(archive_name);
    let staging = root.join("staging");
    std::fs::create_dir_all(staging.join("fomod")).ok()?;
    std::fs::create_dir_all(staging.join("Data/textures")).ok()?;
    std::fs::write(
        staging.join("fomod/ModuleConfig.xml"),
        r#"<config>
  <requiredInstallFiles>
    <file source="Data/textures/sky.dds" destination="Data/textures/sky.dds" />
  </requiredInstallFiles>
</config>"#,
    )
    .ok()?;
    std::fs::write(staging.join("Data/textures/sky.dds"), b"dds").ok()?;
    let out_file = std::fs::File::create(&archive_path).ok()?;
    sevenz_rust2::compress(staging.as_path(), out_file).ok()?;
    Some(archive_path)
}

fn create_test_7z_with_image(root: &Path, archive_name: &str) -> Option<PathBuf> {
    let archive_path = root.join(archive_name);
    let staging = root.join("staging_img");
    std::fs::create_dir_all(staging.join("fomod/images")).ok()?;
    std::fs::create_dir_all(staging.join("Data/textures")).ok()?;
    std::fs::write(staging.join("fomod/images/preview.png"), b"pngdata").ok()?;
    std::fs::write(staging.join("Data/textures/sky.dds"), b"dds").ok()?;
    let out_file = std::fs::File::create(&archive_path).ok()?;
    sevenz_rust2::compress(staging.as_path(), out_file).ok()?;
    Some(archive_path)
}

#[test]
fn detect_strategy_data_for_loose_files() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[("textures/sky.dds", b"dds"), ("meshes/rock.nif", b"nif")],
    );
    let strategy = detect_strategy(&archive).unwrap();
    assert!(matches!(strategy, InstallStrategy::Data));
}

#[test]
fn detect_strategy_data_for_archive_with_data_folder() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("Data/", b""),
            ("Data/textures/sky.dds", b"dds"),
            ("Data/meshes/rock.nif", b"nif"),
        ],
    );
    let strategy = detect_strategy(&archive).unwrap();
    assert!(matches!(strategy, InstallStrategy::Data));
}

#[test]
fn detect_strategy_non_zip_defaults_to_data() {
    let tmp = tempdir();
    let archive = tmp.join("mod.7z");
    std::fs::write(&archive, b"fake").unwrap();
    let strategy = detect_strategy(&archive).unwrap();
    assert!(matches!(strategy, InstallStrategy::Data));
}

#[test]
fn detect_strategy_non_zip_with_fomod_uses_fomod_strategy() {
    let tmp = tempdir();
    let Some(archive) = create_test_7z(&tmp, "mod.7z") else {
        return;
    };
    let strategy = detect_strategy(&archive).unwrap();
    assert!(matches!(strategy, InstallStrategy::Fomod(_)));
}

#[test]
fn build_data_archive_plan_identifies_bain_layout() {
    let paths = vec!["00 Core/textures/a.dds", "10 Optional/textures/b.dds"];
    let plan = build_data_archive_plan(&paths);
    assert!(matches!(plan, DataArchivePlan::Bain { .. }));
}

#[test]
fn build_data_archive_plan_identifies_root_level_layout() {
    let paths = vec!["Wrapper/d3d11.dll", "Wrapper/enbseries/effect.txt"];
    let plan = build_data_archive_plan(&paths);
    assert!(matches!(plan, DataArchivePlan::ExtractToModRoot { .. }));
}

#[test]
fn build_data_archive_plan_identifies_data_layout() {
    let paths = vec!["MyMod/Data/textures/sky.dds", "MyMod/Data/meshes/rock.nif"];
    let plan = build_data_archive_plan(&paths);
    assert!(matches!(plan, DataArchivePlan::ExtractToData { .. }));
}

#[test]
fn parse_fomod_from_archive_reads_non_zip_module_config() {
    let tmp = tempdir();
    let Some(archive) = create_test_7z(&tmp, "mod.7z") else {
        return;
    };
    let cfg = parse_fomod_from_archive(&archive).unwrap();
    assert!(!cfg.required_files.is_empty());
}

#[test]
fn parse_fomod_from_archive_reads_non_zip_mixed_case_fomod_path() {
    let tmp = tempdir();
    let archive_path = tmp.join("mod_case.7z");
    let staging = tmp.join("staging_case");
    std::fs::create_dir_all(staging.join("FOMod")).unwrap();
    std::fs::create_dir_all(staging.join("Data/textures")).unwrap();
    std::fs::write(
        staging.join("FOMod/ModuleConfig.xml"),
        r#"<config>
  <requiredInstallFiles>
    <file source="Data/textures/sky.dds" destination="Data/textures/sky.dds" />
  </requiredInstallFiles>
</config>"#,
    )
    .unwrap();
    std::fs::write(staging.join("Data/textures/sky.dds"), b"dds").unwrap();
    let out_file = std::fs::File::create(&archive_path).unwrap();
    sevenz_rust2::compress(staging.as_path(), out_file).unwrap();

    let cfg = parse_fomod_from_archive(&archive_path).unwrap();
    assert!(!cfg.required_files.is_empty());
}

#[test]
fn archive_has_data_folder_flat_content() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[("textures/sky.dds", b"dds"), ("plugin.esp", b"esp")],
    );
    assert!(!archive_has_data_folder(&archive));
}

#[test]
fn archive_has_data_folder_direct_data_prefix() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("Data/", b""),
            ("Data/textures/sky.dds", b"dds"),
            ("Data/meshes/rock.nif", b"nif"),
        ],
    );
    assert!(!archive_has_data_folder(&archive));
}

#[test]
fn archive_has_data_folder_wrapped_data() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("MyMod/", b""),
            ("MyMod/Data/", b""),
            ("MyMod/Data/textures/sky.dds", b"dds"),
        ],
    );
    assert!(archive_has_data_folder(&archive));
}

#[test]
fn install_flat_archive_places_files_under_data_subdir() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("textures/sky.dds", b"dds_data"),
            ("plugin.esp", b"esp_data"),
        ],
    );
    let dest = tmp.join("mod_dir");
    std::fs::create_dir_all(&dest).unwrap();
    let data_dest = dest.join("Data");
    std::fs::create_dir_all(&data_dest).unwrap();
    extract_zip_to(&archive, &data_dest, "", &|_, _| true).unwrap();

    assert!(dest.join("Data").join("textures").join("sky.dds").exists());
    assert!(dest.join("Data").join("plugin.esp").exists());
    assert_eq!(
        std::fs::read_to_string(dest.join("Data").join("plugin.esp")).unwrap(),
        "esp_data"
    );
}

#[test]
fn install_data_folder_archive_preserves_data_subdir() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[("Data/", b""), ("Data/textures/sky.dds", b"dds_data")],
    );
    let dest = tmp.join("mod_dir");
    std::fs::create_dir_all(&dest).unwrap();
    assert!(!archive_has_data_folder(&archive));
    let data_dest = dest.join("Data");
    std::fs::create_dir_all(&data_dest).unwrap();
    extract_zip_to(&archive, &data_dest, "Data/", &|_, _| true).unwrap();

    assert!(dest.join("Data").join("textures").join("sky.dds").exists());
}

#[test]
fn install_wrapped_data_archive_preserves_data_subdir() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("MyMod/", b""),
            ("MyMod/Data/", b""),
            ("MyMod/Data/textures/sky.dds", b"dds_data"),
        ],
    );
    let dest = tmp.join("mod_dir");
    std::fs::create_dir_all(&dest).unwrap();
    assert!(archive_has_data_folder(&archive));
    extract_zip_to(&archive, &dest, "MyMod/", &|_, _| true).unwrap();

    assert!(dest.join("Data").join("textures").join("sky.dds").exists());
}

#[test]
fn extract_zip_strips_common_prefix() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("MyMod/", b""),
            ("MyMod/textures/sky.dds", b"dds_data"),
            ("MyMod/plugin.esp", b"esp_data"),
        ],
    );
    let dest = tmp.join("extracted");
    std::fs::create_dir_all(&dest).unwrap();
    extract_zip_to(&archive, &dest, "MyMod/", &|_, _| true).unwrap();
    assert!(dest.join("textures").join("sky.dds").exists());
    assert!(dest.join("plugin.esp").exists());
}

#[test]
fn is_safe_relative_path_rejects_traversal() {
    assert!(!is_safe_relative_path("../etc/passwd"));
    assert!(!is_safe_relative_path("foo/../../bar"));
    assert!(!is_safe_relative_path("/absolute/path"));
    assert!(is_safe_relative_path("foo/bar/baz"));
    assert!(is_safe_relative_path("textures/sky.dds"));
    assert!(is_safe_relative_path("a/../a/b"));
}

#[test]
fn strip_data_prefix_removes_leading_data_segment() {
    assert_eq!(strip_data_prefix("Data"), "");
    assert_eq!(strip_data_prefix("data"), "");
    assert_eq!(strip_data_prefix("DATA"), "");
    assert_eq!(strip_data_prefix("Data/"), "");
    assert_eq!(strip_data_prefix("Data/textures"), "textures");
    assert_eq!(strip_data_prefix("data/Textures/sky"), "Textures/sky");
    assert_eq!(strip_data_prefix("DATA/meshes/rock.nif"), "meshes/rock.nif");
    assert_eq!(strip_data_prefix("textures/sky.dds"), "textures/sky.dds");
    assert_eq!(strip_data_prefix(""), "");
    assert_eq!(strip_data_prefix("SomeOtherFolder"), "SomeOtherFolder");
}

#[test]
fn install_fomod_files_falls_back_when_source_uses_data_prefix() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("fomod/", b""),
            ("fomod/ModuleConfig.xml", b"<config/>"),
            ("textures/sky.dds", b"dds_data"),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[FomodFile {
            source: "Data/textures".to_string(),
            destination: "Data/textures".to_string(),
            priority: 0,
        }],
    )
    .unwrap();

    assert!(dest.join("textures").join("sky.dds").exists());
}

#[test]
fn install_fomod_files_handles_source_with_trailing_slash() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("meshes/", b""),
            ("meshes/armor.nif", b"nif_data"),
            ("fomod/", b""),
            ("fomod/ModuleConfig.xml", b"<config/>"),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[FomodFile {
            source: "meshes/".to_string(),
            destination: "Data/meshes".to_string(),
            priority: 0,
        }],
    )
    .unwrap();

    assert!(dest.join("meshes").join("armor.nif").exists());
}

#[test]
fn install_fomod_files_data_root_fallback_skips_fomod_config_dir() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("fomod/", b""),
            ("fomod/ModuleConfig.xml", b"<config/>"),
            ("plugin.esp", b"esp_data"),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[FomodFile {
            source: "Data".to_string(),
            destination: "Data".to_string(),
            priority: 0,
        }],
    )
    .unwrap();

    assert!(dest.join("plugin.esp").exists());
    assert!(!dest.join("fomod").exists());
}

#[test]
fn install_fomod_preserves_case_with_data_root_fallback() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("fomod/", b""),
            ("fomod/ModuleConfig.xml", b"<config/>"),
            ("FileName.ESP", b"esp_data"),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[FomodFile {
            source: "Data".to_string(),
            destination: "Data".to_string(),
            priority: 0,
        }],
    )
    .unwrap();

    assert!(dest.join("FileName.ESP").exists());
}

#[test]
fn install_fomod_files_skips_empty_directory_entries() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("textures/", b""),
            ("textures/armor.dds", b"dds_data"),
            ("textures/empty/", b""),
            ("fomod/", b""),
            ("fomod/ModuleConfig.xml", b"<config/>"),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[FomodFile {
            source: "textures".to_string(),
            destination: "Data/textures".to_string(),
            priority: 0,
        }],
    )
    .unwrap();

    assert!(dest.join("textures").join("armor.dds").exists());
    assert!(!dest.join("textures").join("empty").exists());
}

#[test]
fn install_fomod_files_matches_sources_in_wrapped_archives() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("Aela Replacer/", b""),
            ("Aela Replacer/00 main/", b""),
            ("Aela Replacer/00 main/AelaStandalone.esp", b"esp_data"),
            (
                "Aela Replacer/00 main/textures/Actors/Character/Aela/Head/femalehead.dds",
                b"dds_data",
            ),
            ("Aela Replacer/fomod/", b""),
            ("Aela Replacer/fomod/ModuleConfig.xml", b"<config/>"),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[FomodFile {
            source: "00 main".to_string(),
            destination: "Data".to_string(),
            priority: 0,
        }],
    )
    .unwrap();

    assert!(dest.join("AelaStandalone.esp").exists());
    assert!(
        dest.join("textures")
            .join("Actors")
            .join("Character")
            .join("Aela")
            .join("Head")
            .join("femalehead.dds")
            .exists()
    );
}

#[test]
fn parse_fomod_xml_parses_plugin_dependencies_flags_and_image() {
    let xml = br#"
        <config>
          <moduleName>Example Mod</moduleName>
          <installSteps>
            <installStep name="Variants">
              <optionalFileGroups>
                <group name="Main" type="SelectAny">
                  <plugins>
                    <plugin name="Plus Variant">
                      <description>Use plus variant</description>
                      <image path="images/plus.png"/>
                      <conditionFlags>
                        <flag name="VariantSign">+</flag>
                      </conditionFlags>
                      <dependencies operator="And">
                        <flagDependency flag="FeaturePack" value="+"/>
                      </dependencies>
                      <files>
                        <file source="plus/file.txt" destination="Data/file.txt"/>
                      </files>
                    </plugin>
                  </plugins>
                </group>
              </optionalFileGroups>
            </installStep>
          </installSteps>
        </config>
    "#;

    let cfg = parse_fomod_xml(xml).unwrap();
    let plugin = &cfg.steps[0].groups[0].plugins[0];
    assert_eq!(plugin.image_path.as_deref(), Some("images/plus.png"));
    assert_eq!(
        plugin.condition_flags,
        vec![ConditionFlag {
            name: "VariantSign".to_string(),
            value: "+".to_string(),
        }]
    );
    assert_eq!(
        plugin.dependencies,
        Some(PluginDependencies {
            operator: DependencyOperator::And,
            flags: vec![FlagDependency {
                flag: "FeaturePack".to_string(),
                value: "+".to_string(),
            }],
        })
    );
    assert!(cfg.steps[0].visible.is_none());
    assert!(cfg.conditional_file_installs.is_empty());
}

#[test]
fn parse_fomod_xml_parses_step_visibility_and_conditional_files() {
    let xml = br#"
        <config>
          <moduleName>Example Mod</moduleName>
          <installSteps>
            <installStep name="Underwear Options">
              <visible>
                <flagDependency flag="bUnderwear" value="On" />
              </visible>
              <optionalFileGroups>
                <group name="Color" type="SelectExactlyOne">
                  <plugins>
                    <plugin name="Black">
                      <files>
                        <folder source="16 Underwear" destination="" priority="0"/>
                      </files>
                    </plugin>
                  </plugins>
                </group>
              </optionalFileGroups>
            </installStep>
          </installSteps>
          <conditionalFileInstalls>
            <patterns>
              <pattern>
                <dependencies>
                  <flagDependency flag="bUnderwear" value="On"/>
                </dependencies>
                <files>
                  <folder source="22 Underwear Dark Purple" destination=""/>
                </files>
              </pattern>
            </patterns>
          </conditionalFileInstalls>
        </config>
    "#;

    let cfg = parse_fomod_xml(xml).unwrap();
    assert_eq!(cfg.steps.len(), 1);
    assert_eq!(
        cfg.steps[0].visible,
        Some(PluginDependencies {
            operator: DependencyOperator::And,
            flags: vec![FlagDependency {
                flag: "bUnderwear".to_string(),
                value: "On".to_string(),
            }],
        })
    );
    assert_eq!(cfg.conditional_file_installs.len(), 1);
    assert_eq!(
        cfg.conditional_file_installs[0].dependencies,
        PluginDependencies {
            operator: DependencyOperator::And,
            flags: vec![FlagDependency {
                flag: "bUnderwear".to_string(),
                value: "On".to_string(),
            }],
        }
    );
    assert_eq!(cfg.conditional_file_installs[0].files.len(), 1);
    assert_eq!(
        cfg.conditional_file_installs[0].files[0].source,
        "22 Underwear Dark Purple"
    );
}

#[test]
fn read_archive_file_bytes_finds_fomod_relative_image_path() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("MyMod/", b""),
            ("MyMod/fomod/", b""),
            ("MyMod/fomod/images/preview.png", b"pngdata"),
        ],
    );
    let bytes = read_archive_file_bytes(&archive, "images/preview.png").unwrap();
    assert_eq!(bytes, b"pngdata");
}

#[test]
fn read_archive_file_bytes_non_zip_finds_image() {
    let tmp = tempdir();
    let Some(archive) = create_test_7z_with_image(&tmp, "mod_img.7z") else {
        return;
    };
    let bytes = read_archive_file_bytes(&archive, "images/preview.png").unwrap();
    assert_eq!(bytes, b"pngdata");
}

#[test]
fn read_archive_files_bytes_non_zip_batch_single_pass() {
    let tmp = tempdir();
    let staging = tmp.join("staging_batch");
    std::fs::create_dir_all(staging.join("fomod/images")).unwrap();
    std::fs::create_dir_all(staging.join("Data/textures")).unwrap();
    std::fs::write(staging.join("fomod/images/a.png"), b"aaa").unwrap();
    std::fs::write(staging.join("fomod/images/b.png"), b"bbb").unwrap();
    std::fs::write(staging.join("Data/textures/sky.dds"), b"dds").unwrap();
    let archive_path = tmp.join("batch.7z");
    let out_file = std::fs::File::create(&archive_path).unwrap();
    if sevenz_rust2::compress(staging.as_path(), out_file).is_err() {
        return;
    }

    let result =
        read_archive_files_bytes(&archive_path, &["images/a.png", "images/b.png"]).unwrap();
    assert_eq!(result.len(), 2, "expected both images to be read");
    assert_eq!(result.get("images/a.png").unwrap(), b"aaa");
    assert_eq!(result.get("images/b.png").unwrap(), b"bbb");
}

#[test]
fn install_fomod_files_non_zip_installs_selected_files() {
    let tmp = tempdir();
    let Some(archive) = create_test_7z(&tmp, "mod_fomod.7z") else {
        return;
    };
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[FomodFile {
            source: "textures".to_string(),
            destination: "Data/textures".to_string(),
            priority: 0,
        }],
    )
    .unwrap();

    assert!(dest.join("textures").join("sky.dds").exists());
    assert_eq!(
        std::fs::read(dest.join("textures").join("sky.dds")).unwrap(),
        b"dds",
    );
}

#[test]
fn install_fomod_files_from_dir_same_result_as_zip() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("fomod/", b""),
            ("fomod/ModuleConfig.xml", b"<config/>"),
            ("textures/sky.dds", b"dds_data"),
        ],
    );
    let dest_zip = tmp.join("dest_zip");
    std::fs::create_dir_all(&dest_zip).unwrap();
    let files = vec![FomodFile {
        source: "textures".to_string(),
        destination: "Data/textures".to_string(),
        priority: 0,
    }];
    install_fomod_files(&archive, &dest_zip, &files).unwrap();

    let extracted = tmp.join("extracted");
    std::fs::create_dir_all(extracted.join("fomod")).unwrap();
    std::fs::write(extracted.join("fomod/ModuleConfig.xml"), b"<config/>").unwrap();
    std::fs::create_dir_all(extracted.join("textures")).unwrap();
    std::fs::write(extracted.join("textures/sky.dds"), b"dds_data").unwrap();

    let dest_dir = tmp.join("dest_dir");
    std::fs::create_dir_all(&dest_dir).unwrap();
    super::install::install_fomod_files_from_dir(&extracted, &dest_dir, &files).unwrap();

    assert!(dest_dir.join("textures").join("sky.dds").exists());
    assert_eq!(
        std::fs::read(dest_zip.join("textures").join("sky.dds")).unwrap(),
        std::fs::read(dest_dir.join("textures").join("sky.dds")).unwrap(),
    );
}

#[test]
fn install_fomod_files_normalizes_uppercase_dirs_to_lowercase() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("fomod/", b""),
            ("fomod/ModuleConfig.xml", b"<config/>"),
            ("CalienteTools/BodySlide/SliderSets/CBBE.osp", b"osp_data"),
            ("TEXTURES/actors/character/cbbe.dds", b"dds_data"),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[
            FomodFile {
                source: "CalienteTools".to_string(),
                destination: "Data/CalienteTools".to_string(),
                priority: 0,
            },
            FomodFile {
                source: "TEXTURES".to_string(),
                destination: "Data/TEXTURES".to_string(),
                priority: 0,
            },
        ],
    )
    .unwrap();
    normalize_paths_to_lowercase(&dest);

    assert!(
        dest.join("calientetools")
            .join("bodyslide")
            .join("slidersets")
            .join("cbbe.osp")
            .exists(),
        "CalienteTools directory should be normalized to calientetools"
    );
    assert!(
        !dest.join("CalienteTools").exists(),
        "original CalienteTools dir should be gone after normalization"
    );
    assert!(
        dest.join("textures")
            .join("actors")
            .join("character")
            .join("cbbe.dds")
            .exists(),
        "TEXTURES directory should be normalized to textures"
    );
    assert!(
        !dest.join("TEXTURES").exists(),
        "original TEXTURES dir should be gone after normalization"
    );
}

fn tempdir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static CTR: AtomicU32 = AtomicU32::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("linkmm_test_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ── normalize_paths_to_lowercase ─────────────────────────────────────────

#[test]
fn normalize_paths_to_lowercase_renames_uppercase_dirs() {
    let tmp = tempdir();
    std::fs::create_dir_all(tmp.join("TEXTURES")).unwrap();
    std::fs::write(tmp.join("TEXTURES/sky.dds"), b"dds").unwrap();

    normalize_paths_to_lowercase(&tmp);

    assert!(tmp.join("textures").is_dir());
    assert!(tmp.join("textures/sky.dds").exists());
    assert!(!tmp.join("TEXTURES").exists());
}

#[test]
fn normalize_paths_to_lowercase_merges_duplicate_dirs() {
    let tmp = tempdir();
    std::fs::create_dir_all(tmp.join("TEXTURES")).unwrap();
    std::fs::write(tmp.join("TEXTURES/sky.dds"), b"upper").unwrap();
    std::fs::create_dir_all(tmp.join("textures")).unwrap();
    std::fs::write(tmp.join("textures/ground.dds"), b"lower").unwrap();

    normalize_paths_to_lowercase(&tmp);

    assert!(tmp.join("textures/sky.dds").exists());
    assert!(tmp.join("textures/ground.dds").exists());
    assert!(!tmp.join("TEXTURES").exists());
}

#[test]
fn normalize_paths_to_lowercase_recurses_into_subdirs() {
    let tmp = tempdir();
    std::fs::create_dir_all(tmp.join("meshes/ARMOR")).unwrap();
    std::fs::write(tmp.join("meshes/ARMOR/helm.nif"), b"nif").unwrap();

    normalize_paths_to_lowercase(&tmp);

    assert!(tmp.join("meshes/armor/helm.nif").exists());
    assert!(!tmp.join("meshes/ARMOR").exists());
}

#[test]
fn install_flat_archive_normalizes_uppercase_folder_names() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("TEXTURES/", b""),
            ("TEXTURES/sky.dds", b"dds_data"),
            ("meshes/helm.nif", b"nif_data"),
        ],
    );
    let dest = tmp.join("mod_dir");
    std::fs::create_dir_all(&dest).unwrap();
    let data_dest = dest.join("Data");
    std::fs::create_dir_all(&data_dest).unwrap();
    extract_zip_to(&archive, &data_dest, "", &|_, _| true).unwrap();
    normalize_paths_to_lowercase(&data_dest);

    assert!(data_dest.join("textures").join("sky.dds").exists());
    assert!(!data_dest.join("TEXTURES").exists());
}

#[test]
fn detect_strategy_zip_fomod_with_backslash_separator() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("fomod\\ModuleConfig.xml", b"<config/>"),
            ("Data\\textures\\sky.dds", b"dds"),
        ],
    );
    let strategy = detect_strategy(&archive).unwrap();
    assert!(
        matches!(strategy, InstallStrategy::Fomod(_)),
        "expected Fomod strategy for zip with backslash paths, got Data"
    );
}

#[test]
fn detect_strategy_zip_fomod_wrapped_backslash() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("MyMod\\fomod\\ModuleConfig.xml", b"<config/>"),
            ("MyMod\\Data\\textures\\sky.dds", b"dds"),
        ],
    );
    let strategy = detect_strategy(&archive).unwrap();
    assert!(
        matches!(strategy, InstallStrategy::Fomod(_)),
        "expected Fomod strategy for wrapped zip with backslash paths, got Data"
    );
}

#[test]
fn find_data_root_returns_fomod_wrapper_not_variant_subdir() {
    let paths = &[
        "MyMod/fomod/ModuleConfig.xml",
        "MyMod/Option A/textures/body.dds",
        "MyMod/Option B/textures/body_alt.dds",
    ];
    let root = find_data_root_in_paths(paths);
    assert_eq!(
        root, "MyMod/",
        "expected FOMOD wrapper dir as root, got {root:?}"
    );
}

#[test]
fn find_data_root_fomod_at_archive_root_returns_empty() {
    let paths = &["fomod/ModuleConfig.xml", "Option A/textures/body.dds"];
    let root = find_data_root_in_paths(paths);
    assert_eq!(
        root, "",
        "FOMOD at archive root should yield empty data root, got {root:?}"
    );
}

fn create_diamond_skin_zip(dir: &Path) -> PathBuf {
    let fomod_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<config xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
    xsi:noNamespaceSchemaLocation="http://qconsulting.ca/fo3/ModConfig5.0.xsd">
    <moduleName>Diamond Skin</moduleName>
    <installSteps order="Explicit">
        <installStep name="Choose Body Type">
            <optionalFileGroups order="Explicit">
                <group name="Body Type" type="SelectExactlyOne">
                    <plugins order="Explicit">
                        <plugin name="CBBE">
                            <description>CBBE textures</description>
                            <conditionFlags>
                                <flag name="isCBBE">selected</flag>
                            </conditionFlags>
                            <typeDescriptor>
                                <type name="Recommended"/>
                            </typeDescriptor>
                        </plugin>
                        <plugin name="UNP">
                            <description>UNP textures</description>
                            <conditionFlags>
                                <flag name="isUNP">selected</flag>
                            </conditionFlags>
                            <typeDescriptor>
                                <type name="Optional"/>
                            </typeDescriptor>
                        </plugin>
                    </plugins>
                </group>
                <group name="Texture Resolution" type="SelectExactlyOne">
                    <plugins order="Explicit">
                        <plugin name="4K">
                            <description>4K resolution</description>
                            <conditionFlags>
                                <flag name="res">4k</flag>
                            </conditionFlags>
                            <typeDescriptor>
                                <type name="Recommended"/>
                            </typeDescriptor>
                        </plugin>
                        <plugin name="2K">
                            <description>2K resolution</description>
                            <conditionFlags>
                                <flag name="res">2k</flag>
                            </conditionFlags>
                            <typeDescriptor>
                                <type name="Optional"/>
                            </typeDescriptor>
                        </plugin>
                    </plugins>
                </group>
            </optionalFileGroups>
        </installStep>
    </installSteps>
    <conditionalFileInstalls>
        <patterns>
            <pattern>
                <dependencies operator="And">
                    <flagDependency flag="isCBBE" value="selected"/>
                    <flagDependency flag="res" value="4k"/>
                </dependencies>
                <files>
                    <folder source="CBBE 4K" destination=""/>
                </files>
            </pattern>
            <pattern>
                <dependencies operator="And">
                    <flagDependency flag="isCBBE" value="selected"/>
                    <flagDependency flag="res" value="2k"/>
                </dependencies>
                <files>
                    <folder source="CBBE 2K" destination=""/>
                </files>
            </pattern>
            <pattern>
                <dependencies operator="And">
                    <flagDependency flag="isUNP" value="selected"/>
                    <flagDependency flag="res" value="4k"/>
                </dependencies>
                <files>
                    <folder source="UNP 4K" destination=""/>
                </files>
            </pattern>
        </patterns>
    </conditionalFileInstalls>
</config>"#;
    create_test_zip(
        dir,
        &[
            ("fomod/ModuleConfig.xml", fomod_xml.as_bytes()),
            ("fomod/images/cbbe.png", b"png"),
            (
                "CBBE 4K/textures/actors/character/female/femalebody_1.dds",
                b"cbbe4k",
            ),
            (
                "CBBE 2K/textures/actors/character/female/femalebody_1.dds",
                b"cbbe2k",
            ),
            (
                "UNP 4K/textures/actors/character/female/femalebody_1.dds",
                b"unp4k",
            ),
        ],
    )
}

#[test]
fn detect_strategy_diamond_skin_style_zip_uses_fomod_strategy() {
    let tmp = tempdir();
    let archive = create_diamond_skin_zip(&tmp);
    let strategy = detect_strategy(&archive).unwrap();
    assert!(
        matches!(strategy, InstallStrategy::Fomod(_)),
        "expected Fomod strategy for Diamond Skin-style ZIP"
    );
}

#[test]
fn parse_fomod_xml_diamond_skin_conditional_files_pattern() {
    let tmp = tempdir();
    let archive = create_diamond_skin_zip(&tmp);
    let config = parse_fomod_from_zip(&archive).unwrap();

    assert_eq!(config.mod_name.as_deref(), Some("Diamond Skin"));
    assert!(config.required_files.is_empty());
    assert_eq!(config.steps.len(), 1);

    let step = &config.steps[0];
    assert_eq!(step.groups.len(), 2);

    let body_group = &step.groups[0];
    assert_eq!(body_group.plugins.len(), 2);

    let cbbe = &body_group.plugins[0];
    assert_eq!(cbbe.name, "CBBE");
    assert!(cbbe.files.is_empty());
    assert_eq!(cbbe.condition_flags.len(), 1);
    assert_eq!(cbbe.condition_flags[0].name, "isCBBE");
    assert_eq!(cbbe.condition_flags[0].value, "selected");

    assert!(config.conditional_file_installs.len() >= 3);

    let first = &config.conditional_file_installs[0];
    assert_eq!(first.dependencies.flags.len(), 2);
    assert_eq!(first.files.len(), 1);
    assert_eq!(first.files[0].source, "CBBE 4K");
    assert_eq!(first.files[0].destination, "");
}

#[test]
fn install_fomod_files_diamond_skin_conditional_pattern() {
    let tmp = tempdir();
    let archive = create_diamond_skin_zip(&tmp);
    let dest = tmp.join("Data");
    std::fs::create_dir_all(&dest).unwrap();

    let files = vec![FomodFile {
        source: "CBBE 4K".to_string(),
        destination: String::new(),
        priority: 0,
    }];
    install_fomod_files(&archive, &dest, &files).unwrap();

    assert!(
        dest.join("textures/actors/character/female/femalebody_1.dds")
            .exists()
    );
    assert!(!dest.join("CBBE 4K").exists());
    assert!(!dest.join("CBBE 2K").exists());
}

// ── Encoding detection tests ──────────────────────────────────────────────

fn to_utf16_le_with_bom(s: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for ch in s.encode_utf16() {
        out.extend_from_slice(&ch.to_le_bytes());
    }
    out
}

fn to_utf16_be_with_bom(s: &str) -> Vec<u8> {
    let mut out = vec![0xFE, 0xFF];
    for ch in s.encode_utf16() {
        out.extend_from_slice(&ch.to_be_bytes());
    }
    out
}

#[test]
fn decode_fomod_xml_handles_utf16_le_bom() {
    let xml = r#"<?xml version="1.0" encoding="UTF-16"?>
<config>
  <moduleName>My Mod</moduleName>
</config>"#;
    let encoded = to_utf16_le_with_bom(xml);
    let result = decode_fomod_xml(&encoded).unwrap();
    assert!(result.contains("My Mod"));
}

#[test]
fn decode_fomod_xml_handles_utf16_be_bom() {
    let xml = r#"<?xml version="1.0" encoding="UTF-16"?>
<config>
  <moduleName>My Mod BE</moduleName>
</config>"#;
    let encoded = to_utf16_be_with_bom(xml);
    let result = decode_fomod_xml(&encoded).unwrap();
    assert!(result.contains("My Mod BE"));
}

#[test]
fn decode_fomod_xml_strips_utf8_bom() {
    let xml = "<config><moduleName>Plain</moduleName></config>";
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(xml.as_bytes());
    let result = decode_fomod_xml(&bytes).unwrap();
    assert!(result.contains("Plain"));
    assert!(!result.starts_with('\u{FEFF}'));
}

#[test]
fn decode_fomod_xml_handles_plain_utf8() {
    let xml = "<config><moduleName>Plain UTF-8</moduleName></config>";
    let result = decode_fomod_xml(xml.as_bytes()).unwrap();
    assert_eq!(result, xml);
}

#[test]
fn parse_fomod_xml_accepts_utf16_le_encoded_bytes() {
    let xml = r#"<config xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <moduleName>Diamond Mod</moduleName>
  <installSteps>
    <installStep name="Step1">
      <optionalFileGroups>
        <group name="Options" type="SelectAll">
          <plugins>
            <plugin name="Option A">
              <description>Option A description</description>
              <files>
                <file source="optionA/file.esp" destination="Data/file.esp"/>
              </files>
            </plugin>
          </plugins>
        </group>
      </optionalFileGroups>
    </installStep>
  </installSteps>
</config>"#;
    let encoded = to_utf16_le_with_bom(xml);
    let config = parse_fomod_xml(&encoded).unwrap();
    assert_eq!(config.mod_name.as_deref(), Some("Diamond Mod"));
    assert_eq!(config.steps.len(), 1);
    assert_eq!(config.steps[0].groups[0].plugins.len(), 1);
}

#[test]
fn parse_fomod_from_archive_uses_archive_name_as_fallback_for_empty_mod_name() {
    let tmp = tempdir();
    let xml = br#"<config>
  <moduleName></moduleName>
  <installSteps>
    <installStep name="Step1">
      <optionalFileGroups>
        <group name="G" type="SelectAny">
          <plugins>
            <plugin name="P">
              <description>D</description>
              <files><file source="f.esp" destination=""/></files>
            </plugin>
          </plugins>
        </group>
      </optionalFileGroups>
    </installStep>
  </installSteps>
</config>"#;
    let archive_path = tmp.join("MyGreatMod.zip");
    let file = std::fs::File::create(&archive_path).unwrap();
    let mut zip_writer = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip_writer
        .start_file("fomod/ModuleConfig.xml", options)
        .unwrap();
    zip_writer.write_all(xml).unwrap();
    zip_writer.finish().unwrap();

    let config = parse_fomod_from_archive(&archive_path).unwrap();
    assert_eq!(
        config.mod_name.as_deref(),
        Some("MyGreatMod"),
        "should fall back to archive stem when moduleName is empty"
    );
}

#[test]
fn decode_fomod_xml_rejects_odd_length_utf16_le() {
    let bad: Vec<u8> = vec![0xFF, 0xFE, 0x41, 0x00, 0x42];
    let result = decode_fomod_xml(&bad);
    assert!(result.is_err());
}

#[test]
fn decode_fomod_xml_rejects_odd_length_utf16_be() {
    let bad: Vec<u8> = vec![0xFE, 0xFF, 0x00, 0x41, 0x00];
    let result = decode_fomod_xml(&bad);
    assert!(result.is_err());
}

// ── Tests from installer_new.rs ───────────────────────────────────────────

#[test]
fn test_normalize_path_lowercase() {
    assert_eq!(
        normalize_path_lowercase("Textures\\Armor\\Helmet.DDS"),
        "textures/armor/helmet.dds"
    );
    assert_eq!(
        normalize_path_lowercase("Data/meshes/MyMod.NIF"),
        "data/meshes/mymod.nif"
    );
}

#[test]
fn test_strip_data_prefix() {
    assert_eq!(strip_data_prefix("Data/meshes/file.nif"), "meshes/file.nif");
    assert_eq!(
        strip_data_prefix("data/textures/tex.dds"),
        "textures/tex.dds"
    );
    assert_eq!(strip_data_prefix("meshes/file.nif"), "meshes/file.nif");
    assert_eq!(strip_data_prefix("Data"), "");
    assert_eq!(strip_data_prefix("data"), "");
}

#[test]
fn test_is_safe_relative_path() {
    assert!(is_safe_relative_path("meshes/armor/helmet.nif"));
    assert!(is_safe_relative_path("./meshes/file.nif"));
    assert!(!is_safe_relative_path("../../../etc/passwd"));
    assert!(!is_safe_relative_path("/etc/passwd"));
    assert!(is_safe_relative_path("folder/../other/file.txt"));
}

#[test]
fn test_score_as_data_root_empty_prefix() {
    let paths = vec![
        "meshes/armor/helmet.nif".to_string(),
        "textures/armor/helmet.dds".to_string(),
        "MyMod.esp".to_string(),
    ];

    let score = score_as_data_root_owned("", &paths);
    assert_eq!(score, 35);
}

#[test]
fn test_score_as_data_root_with_prefix() {
    let paths = vec![
        "MyMod/meshes/file.nif".to_string(),
        "MyMod/textures/file.dds".to_string(),
        "MyMod/MyMod.esp".to_string(),
    ];

    let score = score_as_data_root_owned("MyMod", &paths);
    assert_eq!(score, 35);
}

#[test]
fn test_score_as_data_root_data_named_dir() {
    let paths = vec![
        "Data/meshes/file.nif".to_string(),
        "Data/MyMod.esp".to_string(),
    ];

    let score = score_as_data_root_owned("Data", &paths);
    assert_eq!(score, 45);
}

#[test]
fn test_detect_data_root_simple() {
    let paths = vec![
        "meshes/armor/helmet.nif".to_string(),
        "textures/armor/helmet.dds".to_string(),
        "MyMod.esp".to_string(),
    ];

    let root = detect_data_root(&paths);
    assert_eq!(root, Some(String::new()));
}

#[test]
fn test_detect_data_root_single_wrapper() {
    let paths = vec![
        "MyMod/meshes/file.nif".to_string(),
        "MyMod/textures/file.dds".to_string(),
        "MyMod/MyMod.esp".to_string(),
    ];

    let root = detect_data_root(&paths);
    assert_eq!(root, Some("mymod".to_string()));
}

#[test]
fn test_detect_data_root_explicit_data_dir() {
    let paths = vec![
        "Data/meshes/file.nif".to_string(),
        "Data/MyMod.esp".to_string(),
    ];

    let root = detect_data_root(&paths);
    assert_eq!(root, Some("data".to_string()));
}

#[test]
fn test_resolve_file_conflicts() {
    let files = vec![
        FomodFile {
            source: "a.txt".to_string(),
            destination: "file.txt".to_string(),
            priority: 0,
        },
        FomodFile {
            source: "b.txt".to_string(),
            destination: "file.txt".to_string(),
            priority: 10,
        },
        FomodFile {
            source: "c.txt".to_string(),
            destination: "other.txt".to_string(),
            priority: 0,
        },
    ];

    let resolved = resolve_file_conflicts(files);
    assert_eq!(resolved.len(), 2);

    let file_txt = resolved
        .iter()
        .find(|f| f.destination == "file.txt")
        .unwrap();
    assert_eq!(file_txt.source, "b.txt");

    let other_txt = resolved
        .iter()
        .find(|f| f.destination == "other.txt")
        .unwrap();
    assert_eq!(other_txt.source, "c.txt");
}

#[test]
fn test_resolve_file_conflicts_position_tiebreak() {
    let files = vec![
        FomodFile {
            source: "a.txt".to_string(),
            destination: "file.txt".to_string(),
            priority: 5,
        },
        FomodFile {
            source: "b.txt".to_string(),
            destination: "file.txt".to_string(),
            priority: 5,
        },
    ];

    let resolved = resolve_file_conflicts(files);
    assert_eq!(resolved.len(), 1);

    // Later position wins on tie
    assert_eq!(resolved[0].source, "b.txt");
}

#[test]
fn test_dependency_evaluate_and() {
    let deps = PluginDependencies {
        operator: DependencyOperator::And,
        flags: vec![
            FlagDependency {
                flag: "TEX_QUALITY".to_string(),
                value: "4K".to_string(),
            },
            FlagDependency {
                flag: "BODY_TYPE".to_string(),
                value: "CBBE".to_string(),
            },
        ],
    };

    let mut active = HashMap::new();
    active.insert("tex_quality".to_string(), "4k".to_string());
    active.insert("body_type".to_string(), "cbbe".to_string());

    assert!(deps.evaluate(&active));

    active.remove("body_type");
    assert!(!deps.evaluate(&active));
}

#[test]
fn test_dependency_evaluate_or() {
    let deps = PluginDependencies {
        operator: DependencyOperator::Or,
        flags: vec![
            FlagDependency {
                flag: "OPTION_A".to_string(),
                value: "YES".to_string(),
            },
            FlagDependency {
                flag: "OPTION_B".to_string(),
                value: "YES".to_string(),
            },
        ],
    };

    let mut active = HashMap::new();
    active.insert("option_a".to_string(), "yes".to_string());

    assert!(deps.evaluate(&active));

    active.clear();
    assert!(!deps.evaluate(&active));
}

// ── FOMOD __MACOSX / wrapper-dir regression tests ────────────────────────────

/// A zip that contains both real content and macOS `__MACOSX/` resource-fork
/// entries must still install the correct files.  Before the fix,
/// `find_common_prefix` saw two different top-level dirs and returned `""`,
/// so no archive entries matched the FOMOD source paths.
#[test]
fn install_fomod_succeeds_when_zip_contains_macosx_entries() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            // macOS resource-fork junk
            ("__MACOSX/", b""),
            (
                "__MACOSX/Diamond mod/textures/._femalebody_1_msn.dds",
                b"junk",
            ),
            // real content under a wrapper directory
            ("Diamond mod/", b""),
            ("Diamond mod/fomod/", b""),
            ("Diamond mod/fomod/ModuleConfig.xml", b"<config/>"),
            ("Diamond mod/00main/", b""),
            ("Diamond mod/00main/textures/", b""),
            (
                "Diamond mod/00main/textures/femalebody_1_msn.dds",
                b"dds_data",
            ),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[FomodFile {
            source: "00main/textures".to_string(),
            destination: "textures".to_string(),
            priority: 0,
        }],
    )
    .unwrap();

    assert!(
        dest.join("textures").join("femalebody_1_msn.dds").exists(),
        "real content should be installed even when __MACOSX/ entries are present"
    );
}

/// A zip with a single wrapper directory (Diamond-style) should have its
/// FOMOD source paths resolved relative to that wrapper directory.
#[test]
fn install_fomod_diamond_style_wrapper_dir() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            ("Diamond 3BA Puffy Pussy normal maps/", b""),
            ("Diamond 3BA Puffy Pussy normal maps/fomod/", b""),
            (
                "Diamond 3BA Puffy Pussy normal maps/fomod/ModuleConfig.xml",
                b"<config/>",
            ),
            (
                "Diamond 3BA Puffy Pussy normal maps/00main/textures/femalebody_1_msn.dds",
                b"dds1",
            ),
            (
                "Diamond 3BA Puffy Pussy normal maps/body_n/smooth map/textures/body_smooth.dds",
                b"dds2",
            ),
        ],
    );
    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[
            FomodFile {
                source: "00main\\textures".to_string(),
                destination: "textures".to_string(),
                priority: 0,
            },
            FomodFile {
                source: "body_n\\smooth map\\textures".to_string(),
                destination: "textures".to_string(),
                priority: 1,
            },
        ],
    )
    .unwrap();

    assert!(
        dest.join("textures").join("femalebody_1_msn.dds").exists(),
        "00main/textures content should be installed"
    );
    assert!(
        dest.join("textures").join("body_smooth.dds").exists(),
        "body_n/smooth map/textures content should be installed"
    );
}

// ── find_fomod_parent_dir / two-level wrapper regression tests ────────────────

/// Real-world Diamond archive layout: two-level nesting where the outer dir
/// is the zip filename and the inner dir is the actual content wrapper.
/// Previously failed because find_fomod_parent_dir only returned the outer dir.
#[test]
fn install_fomod_two_level_wrapper_dir() {
    let tmp = tempdir();
    let archive = create_test_zip(
        &tmp,
        &[
            // outer: zip-name directory
            ("Diamond 3BA puffy pussy-45718/", b""),
            // inner: actual content wrapper (different name from outer)
            (
                "Diamond 3BA puffy pussy-45718/Diamond 3BA Puffy Pussy normal maps/",
                b"",
            ),
            (
                "Diamond 3BA puffy pussy-45718/Diamond 3BA Puffy Pussy normal maps/fomod/ModuleConfig.xml",
                b"<config/>",
            ),
            (
                "Diamond 3BA puffy pussy-45718/Diamond 3BA Puffy Pussy normal maps/00main/textures/body.dds",
                b"dds1",
            ),
            (
                "Diamond 3BA puffy pussy-45718/Diamond 3BA Puffy Pussy normal maps/body_n/smooth map/textures/body_smooth.dds",
                b"dds2",
            ),
        ],
    );

    let dest = tmp.join("mod_data");
    std::fs::create_dir_all(&dest).unwrap();

    install_fomod_files(
        &archive,
        &dest,
        &[
            FomodFile {
                source: "00main\\textures".to_string(),
                destination: "textures".to_string(),
                priority: 0,
            },
            FomodFile {
                source: "body_n\\smooth map\\textures".to_string(),
                destination: "textures".to_string(),
                priority: 1,
            },
        ],
    )
    .unwrap();

    assert!(
        dest.join("textures").join("body.dds").exists(),
        "00main/textures content should be installed from two-level wrapper archive"
    );
    assert!(
        dest.join("textures").join("body_smooth.dds").exists(),
        "body_n/ content should be installed from two-level wrapper archive"
    );
}

/// `find_fomod_parent_dir` must return the full multi-component path for
/// archives with two levels of nesting.
#[test]
fn find_fomod_parent_dir_two_level_returns_full_path() {
    let paths = &[
        "outer/inner/fomod/ModuleConfig.xml",
        "outer/inner/00main/textures/body.dds",
    ];
    let parent = find_fomod_parent_dir(paths).unwrap();
    assert_eq!(
        parent, "outer/inner",
        "should return the full two-level parent, got '{parent}'"
    );
}

/// Single-level nesting must still work after the fix.
#[test]
fn find_fomod_parent_dir_single_level_still_works() {
    let paths = &[
        "MyMod/fomod/ModuleConfig.xml",
        "MyMod/00main/textures/body.dds",
    ];
    let parent = find_fomod_parent_dir(paths).unwrap();
    assert_eq!(parent, "MyMod");
}

/// Archive root (no wrapper) must still return empty string.
#[test]
fn find_fomod_parent_dir_at_root_returns_empty() {
    let paths = &["fomod/ModuleConfig.xml", "00main/textures/body.dds"];
    let parent = find_fomod_parent_dir(paths).unwrap();
    assert_eq!(parent, "");
}

/// Extracted-dir equivalent of `install_fomod_two_level_wrapper_dir`:
/// ensures `install_fomod_files_from_dir` handles two levels of directory
/// nesting by using FOMOD-aware prefix detection.
#[test]
fn install_fomod_files_from_dir_two_level_wrapper() {
    let tmp = tempdir();

    // Build an extracted tree with two levels of nesting around the fomod/ dir.
    let extracted = tmp.join("extracted");
    let inner = extracted.join("Diamond 3BA puffy pussy-45718/Diamond 3BA Puffy Pussy normal maps");
    std::fs::create_dir_all(inner.join("fomod")).unwrap();
    std::fs::write(inner.join("fomod/ModuleConfig.xml"), b"<config/>").unwrap();
    std::fs::create_dir_all(inner.join("00main/textures")).unwrap();
    std::fs::write(inner.join("00main/textures/body.dds"), b"dds1").unwrap();
    std::fs::create_dir_all(inner.join("body_n/smooth map/textures")).unwrap();
    std::fs::write(
        inner.join("body_n/smooth map/textures/body_smooth.dds"),
        b"dds2",
    )
    .unwrap();

    let dest = tmp.join("dest_dir");
    std::fs::create_dir_all(&dest).unwrap();

    super::install::install_fomod_files_from_dir(
        &extracted,
        &dest,
        &[
            FomodFile {
                source: "00main\\textures".to_string(),
                destination: "textures".to_string(),
                priority: 0,
            },
            FomodFile {
                source: "body_n\\smooth map\\textures".to_string(),
                destination: "textures".to_string(),
                priority: 1,
            },
        ],
    )
    .unwrap();

    assert!(
        dest.join("textures").join("body.dds").exists(),
        "00main/textures content should be installed from two-level extracted dir"
    );
    assert!(
        dest.join("textures").join("body_smooth.dds").exists(),
        "body_n/ content should be installed from two-level extracted dir"
    );
}

/// Verify that `decode_fomod_xml` falls back to Windows-1252 decoding when
/// the bytes are not valid UTF-8.
#[test]
fn decode_fomod_xml_handles_windows_1252_encoding() {
    // "Möd Nàme" in Windows-1252: ö = 0xF6, à = 0xE0
    let bytes: &[u8] = b"<?xml version=\"1.0\"?>\n<config>M\xF6d N\xE0me</config>";
    let decoded = super::fomod::decode_fomod_xml(bytes).unwrap();
    assert!(
        decoded.contains("Möd Nàme"),
        "Windows-1252 characters should be decoded correctly, got: {decoded}"
    );
}
