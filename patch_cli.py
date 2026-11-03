import io

P = r"E:\KF\trit-claims\crates\trit-vcs\src\bin\trit-vcs.rs"
s = io.open(P, encoding="utf-8").read()

s = s.replace(
    "use trit_vcs::repo::{CommitRequest, Repo};",
    "use trit_vcs::repo::{CommitRequest, Repo};\nuse trit_vcs::worktree;",
    1,
)

commands = '''        "status" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            match worktree::status(&repo) {
                Ok(changes) => {
                    if has(&args, "--json") {
                        println!("{}", serde_json::to_string_pretty(&changes).unwrap_or_default());
                    } else if changes.is_empty() {
                        println!("工作树干净（与 HEAD 逐内容一致）");
                    } else {
                        for change in &changes {
                            println!("  {:?}  {}", change.kind, change.path);
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "checkout" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let Some(target) = args.iter().skip(1).find(|arg| !arg.starts_with("--")) else {
                eprintln!("checkout 需要提交 id");
                return ExitCode::from(2);
            };
            let Some(id) = trit_vcs::Hash::from_hex(target) else {
                return fail(format!("不是合法提交 id：{target}"));
            };
            match worktree::checkout(&repo, &id, has(&args, "--force")) {
                Ok(written) => {
                    println!("已还原 {written} 个文件（{}）", id.short());
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "diff" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let ids: Vec<&str> = args
                .iter()
                .skip(1)
                .filter(|arg| !arg.starts_with("--"))
                .map(|arg| arg.as_str())
                .collect();
            if ids.len() < 2 {
                eprintln!("diff 需要两个提交 id");
                return ExitCode::from(2);
            }
            let (Some(left), Some(right)) = (
                trit_vcs::Hash::from_hex(ids[0]),
                trit_vcs::Hash::from_hex(ids[1]),
            ) else {
                return fail("提交 id 不合法");
            };
            match worktree::diff(&repo, &left, &right) {
                Ok(changes) => {
                    if has(&args, "--json") {
                        println!("{}", serde_json::to_string_pretty(&changes).unwrap_or_default());
                    } else {
                        for change in &changes {
                            println!("  {:?}  {}", change.kind, change.path);
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
'''

anchor = '        "verify" => {'
assert anchor in s
s = s.replace(anchor, commands + anchor, 1)
io.open(P, "w", encoding="utf-8", newline="\n").write(s)
print("cli commands added")
