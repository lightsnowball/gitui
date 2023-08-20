use crate::sync::patches::get_file_diff_patch_and_hunklines;
use crate::sync::repo;
use crate::sync::staging::apply_selection;
use crate::{
	error::Result,
	sync::{diff::DiffLinePosition, RepoPath},
};
use easy_cast::Conv;
use scopetime::scope_time;
use std::path::Path;

/// TODO lightsnowball - bad docs but FI
/// Private method used for tracking untracked file. Without tracking it we cannot actually
/// stage lines, so we must stage it with intent to add (link?) to be able to perform necessary
/// commands.
fn index_entry_for_untracked_file(
	file_path: &str,
) -> git2::IndexEntry {
	// lightsnowball - try to find other way how to add file, should be in this repo somewhere (?)
	// Cannot read file, its not indexed so i guess thats the problem about finding it...
	// eprintln!("Path to file lighty: {}", file_path);
	git2::IndexEntry {
		ctime: git2::IndexTime::new(0, 0),
		mtime: git2::IndexTime::new(0, 0),
		dev: 0,
		ino: 0,
		mode: 0o100644, // https://git-scm.com/docs/index-format check it out
		uid: 0,
		gid: 0,
		file_size: 0,
		id: git2::Oid::hash_file(git2::ObjectType::Blob, file_path)
			.unwrap(),
		flags: git2::IndexEntryFlag::EXTENDED.bits(),
		flags_extended: git2::IndexEntryExtendedFlag::INTENT_TO_ADD
			.bits(),
		path: file_path.bytes().into_iter().collect(),
	}
}

/// Lightsnowball - i think this doesn't work for untracked (component/diff.rs says this, and it
/// really doesn't work, but dont know whose responsibility this is)
/// - Yup, won't work with untracked, not sure if this can be modified or whole new method is needed.
/// - CHECK THIS OUT! It also doesn't work when you want to unstage file line by line :))) last line
/// won't unstage (like newline or smth like that)
pub fn stage_lines(
	repo_path: &RepoPath,
	file_path: &str,
	is_stage: bool,
	lines: &[DiffLinePosition],
) -> Result<()> {
	scope_time!("stage_lines");

	if lines.is_empty() {
		return Ok(());
	}

	let repo = repo(repo_path)?;

	let mut index = repo.index()?;
	index.read(true)?;

	let mut idx = match index.get_path(Path::new(file_path), 0) {
		Some(idx) => idx,
		None => {
			index
				.add_frombuffer(
					&index_entry_for_untracked_file(file_path),
					&[],
				)
				.unwrap();

			index.write()?;
			index.read(true)?;

			index.get_path(Path::new(file_path), 0).unwrap()
		}
	};

	log::trace!("idx value lighty: {:?}", idx);

	let blob = repo.find_blob(idx.id)?;
	let indexed_content = String::from_utf8(blob.content().into())?;

	let new_content = {
		let (_patch, hunks) = get_file_diff_patch_and_hunklines(
			&repo, file_path, is_stage, false,
		)?;

		let old_lines = indexed_content.lines().collect::<Vec<_>>();

		apply_selection(lines, &hunks, &old_lines, is_stage, false)?
	};

	let blob_id = repo.blob(new_content.as_bytes())?;

	idx.id = blob_id;
	idx.file_size = u32::try_conv(new_content.as_bytes().len())?;
	index.add(&idx)?;

	index.write()?;
	index.read(true)?;

	Ok(())
}

#[cfg(test)]
mod test {
	use super::*;
	use crate::sync::{
		diff::get_diff,
		tests::{get_statuses, repo_init, write_commit_file},
		utils::{repo_write_file, stage_add_file},
	};

	#[test]
	fn test_stage() {
		static FILE_1: &str = r"0
";

		static FILE_2: &str = r"0
1
2
3
";

		let (path, repo) = repo_init().unwrap();
		let path: &RepoPath = &path.path().to_str().unwrap().into();

		write_commit_file(&repo, "test.txt", FILE_1, "c1");

		repo_write_file(&repo, "test.txt", FILE_2).unwrap();

		stage_lines(
			path,
			"test.txt",
			false,
			&[DiffLinePosition {
				old_lineno: None,
				new_lineno: Some(2),
			}],
		)
		.unwrap();

		let diff = get_diff(path, "test.txt", true, None).unwrap();

		assert_eq!(diff.lines, 3);
		assert_eq!(&*diff.hunks[0].lines[0].content, "@@ -1 +1,2 @@");
	}

	#[test]
	fn test_panic_stage_no_newline() {
		static FILE_1: &str = r"a = 1
b = 2";

		static FILE_2: &str = r"a = 2
b = 3
c = 4";

		let (path, repo) = repo_init().unwrap();
		let path: &RepoPath = &path.path().to_str().unwrap().into();

		write_commit_file(&repo, "test.txt", FILE_1, "c1");

		repo_write_file(&repo, "test.txt", FILE_2).unwrap();

		stage_lines(
			path,
			"test.txt",
			false,
			&[
				DiffLinePosition {
					old_lineno: Some(1),
					new_lineno: None,
				},
				DiffLinePosition {
					old_lineno: Some(2),
					new_lineno: None,
				},
			],
		)
		.unwrap();

		let diff = get_diff(path, "test.txt", true, None).unwrap();

		assert_eq!(diff.lines, 5);
		assert_eq!(&*diff.hunks[0].lines[0].content, "@@ -1,2 +1 @@");
	}

	#[test]
	fn test_unstage() {
		static FILE_1: &str = r"0
";

		static FILE_2: &str = r"0
1
2
3
";

		let (path, repo) = repo_init().unwrap();
		let path: &RepoPath = &path.path().to_str().unwrap().into();

		write_commit_file(&repo, "test.txt", FILE_1, "c1");

		repo_write_file(&repo, "test.txt", FILE_2).unwrap();

		assert_eq!(get_statuses(path), (1, 0));

		stage_add_file(path, Path::new("test.txt")).unwrap();

		assert_eq!(get_statuses(path), (0, 1));

		let diff_before =
			get_diff(path, "test.txt", true, None).unwrap();

		assert_eq!(diff_before.lines, 5);

		stage_lines(
			path,
			"test.txt",
			true,
			&[DiffLinePosition {
				old_lineno: None,
				new_lineno: Some(2),
			}],
		)
		.unwrap();

		assert_eq!(get_statuses(path), (1, 1));

		let diff = get_diff(path, "test.txt", true, None).unwrap();

		assert_eq!(diff.lines, 4);
	}
}
