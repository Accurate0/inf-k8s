use crate::forgejo::BOT_USERNAME;

pub fn revert_and_push(
    clone_url: &str,
    token: &str,
    merge_sha: &str,
    commit_msg: &str,
    branch_name: &str,
    target_branch: &str,
) -> Result<(), anyhow::Error> {
    let tmp = tempfile::tempdir()?;

    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks
        .credentials(|_url, _username, _allowed| git2::Cred::userpass_plaintext("oauth2", token));

    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks);

    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_opts);
    builder.branch(target_branch);
    let repo = builder.clone(clone_url, tmp.path())?;

    let head_commit = repo.head()?.peel_to_commit()?;

    // Find the merge commit
    let merge_oid = git2::Oid::from_str(merge_sha)?;
    let merge_commit = repo.find_commit(merge_oid)?;

    // revert_commit(revert_commit, our_commit, mainline, opts)
    // our_commit = HEAD (the commit we're applying the revert onto)
    // mainline = 1 for merge commits (first parent is mainline), 0 for regular commits
    let mainline = if merge_commit.parent_count() > 1 {
        1
    } else {
        0
    };
    let mut revert_index = repo.revert_commit(&merge_commit, &head_commit, mainline, None)?;

    if revert_index.has_conflicts() {
        anyhow::bail!("revert has conflicts — manual resolution needed");
    }

    let tree_oid = revert_index.write_tree_to(&repo)?;
    let tree = repo.find_tree(tree_oid)?;
    let sig = git2::Signature::now(BOT_USERNAME, "janitor@git.anurag.sh")?;
    let commit_oid = repo.commit(None, &sig, &sig, commit_msg, &tree, &[&head_commit])?;

    let commit = repo.find_commit(commit_oid)?;
    repo.branch(branch_name, &commit, false)?;

    // Push the new branch
    let mut remote = repo.find_remote("origin")?;
    let mut push_callbacks = git2::RemoteCallbacks::new();
    push_callbacks
        .credentials(|_url, _username, _allowed| git2::Cred::userpass_plaintext("oauth2", token));
    let mut push_opts = git2::PushOptions::new();
    push_opts.remote_callbacks(push_callbacks);
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
    remote.push(&[&refspec], Some(&mut push_opts))?;

    Ok(())
}
