use crate::sync::patches::get_file_diff_patch_and_hunklines;
use crate::sync::repo;
use crate::sync::staging::apply_selection;
use crate::{
	error::{Error, Result},
	sync::{diff::DiffLinePosition, RepoPath},
};
use easy_cast::Conv;
use git2::{
	Index, IndexEntry, IndexEntryExtendedFlag, IndexEntryFlag,
	IndexTime, Oid,
};
use scopetime::scope_time;
use std::path::Path;

/// lightsnowball - docs
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

	let mut idx = if let Some(idx) =
		index.get_path(Path::new(file_path), 0)
	{
		idx
	} else {
		stage_with_intent_to_add(&mut index, file_path)?;

		index.get_path(Path::new(file_path), 0).ok_or(
			Error::Generic(format!("Couldn't get path {file_path}")),
		)?
	};

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
	// lightsnowball - not sure why 4 or what 4, investigation needed...
	// hard-code to 4, otherwise tracked new files won't stage changes
	idx.flags = 4;

	index.add(&idx)?;
	index.write()?;

	// if file doesn't have any staged content, we must reset it so new files can be returned to
	// untracked state
	if new_content.is_empty() {
		repo.reset_default(None, [file_path]).map_err(|_error| {
			Error::Generic(format!("Couldn't reset {file_path}"))
		})?;
	}

	Ok(())
}

/// lightsnowball - todo docs
fn stage_with_intent_to_add(
	index: &mut Index,
	file_path: &str,
) -> Result<()> {
	let index_entry = IndexEntry {
		ctime: IndexTime::new(0, 0),
		mtime: IndexTime::new(0, 0),
		dev: 0,
		ino: 0,
		mode: 0o100_644, // TODO lightsnowball - check if this has to be take from file instead of hardcoding it like this
		uid: 0,
		gid: 0,
		file_size: 0,
		id: Oid::zero(),
		flags: IndexEntryFlag::EXTENDED.bits(),
		flags_extended: IndexEntryExtendedFlag::INTENT_TO_ADD.bits(),
		path: file_path.bytes().collect(),
	};

	index.add_frombuffer(&index_entry, &[]).map_err(|_error| {
		Error::Generic(format!(
			"Failed to start tracking file {file_path}"
		))
	})?;

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

		// lightsnowball - these checks are failing, but not sure why it seems that
		// `asyncgit/src/sync/staging/mod.rs` implementation of `NewFromOldContent::finish()`
		// modification is causing this to fail, however I'm not sure if that is something we want,
		// make sure you check history of this test and why is it implemented that way
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
