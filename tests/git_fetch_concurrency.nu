def --wrapped capture [...args: string] {
    let out = (run-external $args.0 ...($args | skip 1) | complete)
    if $out.exit_code != 0 { error make {msg: $"Command failed: ($args | str join ' ')\n($out.stderr)"} }
    $out.stdout | str trim
}

def wait-for-file [file: path] {
    for _ in 1..300 {
        if ($file | path exists) { return }
        sleep 50ms
    }
    error make {msg: $"Timed out waiting for ($file)"}
}

# Run with the directory containing the jj binary under test.
def main [bin_dir: path, --ignore-working-copy, --non-colocated] {
    let bin_dir = ($bin_dir | path expand)
    $env.JJ_CONFIG = '/dev/null'
    $env.PATH = ([$bin_dir] | append $env.PATH)
    $env.JJ_USER = 'Reproducer'
    $env.JJ_EMAIL = 'repro@example.invalid'
    $env.GIT_CONFIG_NOSYSTEM = '1'
    $env.GIT_CONFIG_GLOBAL = '/dev/null'
    let root = (^mktemp -d | str trim)
    $env.XDG_CONFIG_HOME = ($root | path join config)
    mkdir $env.XDG_CONFIG_HOME
    print $"Fixture: ($root)"
    let remote = ($root | path join remote)
    let local = ($root | path join local)
    capture git init -b main $remote | ignore
    capture git -C $remote config user.name Reproducer | ignore
    capture git -C $remote config user.email repro@example.invalid | ignore
    'A' | save ($remote | path join a)
    capture git -C $remote add . | ignore
    capture git -C $remote commit -m A | ignore
    capture git -C $remote checkout -b old | ignore
    'B' | save ($remote | path join b)
    capture git -C $remote add . | ignore
    capture git -C $remote commit -m B | ignore
    capture git -C $remote checkout main | ignore
    capture jj git clone --colocate $remote $local | ignore
    cd $local
    capture jj new 'main@origin' | ignore
    # Remote branch rename + new descendant; B remains reachable through new C.
    capture git -C $remote checkout -b new old | ignore
    'C' | save ($remote | path join c)
    capture git -C $remote add . | ignore
    capture git -C $remote commit -m C | ignore
    capture git -C $remote branch -D old | ignore

    let fetch_dir = if $non_colocated {
        let sibling = ($root | path join sibling)
        capture jj workspace add $sibling | ignore
        # Make this disposable jj workspace non-colocated, sharing the same repo.
        rm --force ($sibling | path join .git)
        $sibling
    } else { $local }
    let ready = ($root | path join ready)
    let release = ($root | path join release)
    let fetched = ($root | path join fetched.json)
    let read_done = ($root | path join read.json)
    $env.JJ_TEST_READY = $ready
    $env.JJ_TEST_RELEASE = $release
    let hook = ($local | path join .git hooks reference-transaction)
    # Hold the actual Git prune window open. Always expire, including on test failure.
    '#!/opt/homebrew/bin/nu --no-config-file
def main [state: string] {
    let updates = (^cat)
    if $state == "committed" and ($updates | str contains "refs/remotes/origin/old") {
        touch $env.JJ_TEST_READY
        for _ in 1..200 {
            if ($env.JJ_TEST_RELEASE | path exists) { return }
            sleep 50ms
        }
        exit 1
    }
}
    ' | save $hook
    # Use the current Nu executable instead of a machine-specific shebang.
    let hook_text = (open --raw $hook | str replace '/opt/homebrew/bin/nu' $nu.current-exe)
    $hook_text | save --force $hook
    capture chmod +x $hook | ignore
    let fetch_flags = if $ignore_working_copy { ['--ignore-working-copy'] } else { [] }
    job spawn { ^jj -R $fetch_dir ...$fetch_flags git fetch | complete | to json | save $fetched } | ignore
    wait-for-file $ready
    job spawn { ^jj --no-pager log -r @ --no-graph -T commit_id | complete | to json | save $read_done } | ignore
    # A read without snapshotting must remain available during the fetch.
    capture jj --ignore-working-copy log -r @ --no-graph -T commit_id | ignore
    sleep 300ms
    let imported_early = ($read_done | path exists)
    touch $release
    wait-for-file $fetched
    wait-for-file $read_done
    let fetch_result = (open $fetched)
    let read_result = (open $read_done)
    if $fetch_result.exit_code != 0 { error make {msg: $fetch_result.stderr} }
    if $read_result.exit_code != 0 { error make {msg: $read_result.stderr} }
    # Import has resumed after publication, with no divergent operation to merge.
    let status = (^jj --no-pager st | complete)
    if $status.exit_code != 0 { error make {msg: $status.stderr} }
    let files = (capture jj file list -r 'description(substring:"C")' | lines)
    let actual = (capture jj log -r 'description(substring:"C")' --no-graph -T commit_id)
    let original = (capture git -C $remote rev-parse HEAD)
    if $imported_early { error make {msg: 'Background read imported refs before fetch completed'} }
    if $actual != $original or $files != ['a' 'b' 'c'] {
        error make {msg: 'Fetched history was rewritten during concurrent import'}
    }
    if (($read_result.stderr + $status.stderr) | str contains 'Concurrent modification') {
        error make {msg: 'Fetch and background import created divergent operations'}
    }
    # Early fetch errors must also release the lock.
    let failed = (^jj -R $fetch_dir git fetch --remote missing-remote | complete)
    if $failed.exit_code == 0 { error make {msg: 'Expected missing remote to fail'} }
    capture jj st | ignore
    print $"PASS: fetch ignore-working-copy=($ignore_working_copy), non-colocated=($non_colocated); original C and all files preserved"
}
