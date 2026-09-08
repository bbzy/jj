# Verify the features affected by the rebase onto jj 0.45.1.
def main [] {
    cd ($env.FILE_PWD | path dirname)
    $env.INSTA_UPDATE = "no"

    ^cargo check --workspace --all-targets --offline
    if $env.LAST_EXIT_CODE != 0 { exit $env.LAST_EXIT_CODE }

    ^cargo test -p jj-lib --lib --test runner --offline -- gitattributes:: filter:: test_filter:: test_gitmodules:: local_working_copy:: test_local_working_copy_sparse:: test_workspace::
    if $env.LAST_EXIT_CODE != 0 { exit $env.LAST_EXIT_CODE }

    ^cargo test -p jj-cli --test runner --offline -- test_workspaces:: test_git_filters:: test_working_copy:: test_git_colocated:: test_git_init:: test_git_colocation::
    if $env.LAST_EXIT_CODE != 0 { exit $env.LAST_EXIT_CODE }
}
