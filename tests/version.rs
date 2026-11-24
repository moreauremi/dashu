//! `dashu version` : une ligne « dashu <version> <commit> », que dashu-update
//! compare à celle de la version à installer (SPEC, « Installation simple »).

use std::process::Command;

#[test]
fn version_affiche_la_version_et_le_commit() {
    for argument in ["version", "--version"] {
        let sortie = Command::new(env!("CARGO_BIN_EXE_dashu"))
            .arg(argument)
            .output()
            .expect("lancement du binaire");
        assert!(
            sortie.status.success(),
            "{argument} : code {:?}",
            sortie.status
        );

        let texte = String::from_utf8(sortie.stdout).unwrap();
        let ligne = texte.trim_end();
        assert_eq!(ligne.lines().count(), 1, "une seule ligne : {texte:?}");

        let mots: Vec<&str> = ligne.split(' ').collect();
        assert_eq!(mots.len(), 3, "« dashu <version> <commit> » : {ligne:?}");
        assert_eq!(mots[0], "dashu");
        assert_eq!(mots[1], env!("CARGO_PKG_VERSION"));
        // build.rs donne la même valeur au binaire et aux tests : le commit
        // de DASHU_COMMIT à la compilation, « inconnu » sinon.
        assert_eq!(mots[2], env!("DASHU_COMMIT"));
        assert!(!mots[2].is_empty());
    }
}
