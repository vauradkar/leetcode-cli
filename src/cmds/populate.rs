//! Populate command
use kdam::{term, tqdm, BarExt};
use std::fmt::Write as FmtWrite;
use std::io::Write;
use std::{
    collections::HashMap,
    fs::{create_dir_all, File, OpenOptions},
    path::{Path, PathBuf},
};

use super::Command;
use crate::{cache::models::Problem, helper::code_path, Cache, Config, Error};
use async_trait::async_trait;
use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use colored::Colorize;
use std::os::unix::fs::symlink;

static PERCENT_VIEWS: &[&str] = &[
    "0-10", "10-20", "20-30", "30-40", "40-50", "50-60", "60-70", "70-80", "80-90", "90-100",
];

static RUST_DOC_STR_START: &str = "//!";
static RUST_DOC_STR_END: &str = "\n//!\n";

fn write_comment(content: &mut String, comment: &str) {
    if content.trim().is_empty() {
        write!(content, "{}{}", RUST_DOC_STR_START, RUST_DOC_STR_END).unwrap();
    } else {
        write!(
            content,
            "{} {}{}",
            RUST_DOC_STR_START, comment, RUST_DOC_STR_END
        )
        .unwrap();
    }
}

/// Abstract `populate` command
///
/// ```sh
/// leetcode-populate
/// Populate questions
///
/// USAGE:
///     leetcode populate
///
/// FLAGS:
///     -c, --continue_on_error Prints error message and continues to populate
///     -h, --help       Prints help information
///     -V, --version    Prints version information
///
/// ARGS:
///     <id>    question id
/// ```
pub struct PopulateCommand;

fn create_file_header(problem: &Problem, url: &str, question_desc: &str) -> String {
    let mut content = String::new();
    write_comment(&mut content, "# Challenge info");
    write_comment(&mut content, &format!("url: <{}>", url));
    write_comment(&mut content, &format!("percent: {}", problem.percent));
    write_comment(&mut content, &format!("level: {}", problem.display_level()));
    write_comment(&mut content, &format!("category: {}", problem.category));

    write_comment(&mut content, "# Question");
    for q_line in question_desc.lines() {
        write_comment(&mut content, q_line);
    }

    writeln!(content, "// delete the line below to build the solution\n").unwrap();
    write!(content, "#[cfg(feature = \"ignored\")]").unwrap();
    writeln!(content, "mod inner {{").unwrap();

    writeln!(content, "use crate::solutions::Solution;").unwrap();
    writeln!(content).unwrap();
    content
}

fn create_file_footer(content: &mut String) {
    // closing brace for mod inner
    writeln!(content, "mod x{{}}}}").unwrap();
}

fn fix_rust_code(content: &str) -> Result<String, crate::Error> {
    let content = content.replace('\t', "    ");
    let content = content.replace("box: Vec<Vec<char>>", "boxy: Vec<Vec<char>>");
    let syntax_tree = syn::parse_file(&content).map_err(|e| Error::FeatureError(e.to_string()))?;
    Ok(prettyplease::unparse(&syntax_tree))
}

impl PopulateCommand {
    async fn write_file(
        problem: &Problem,
        conf: &Config,
        cache: &Cache,
    ) -> Result<(), crate::Error> {
        use crate::cache::models::Question;

        let test_flag = conf.code.test;

        let p_desc_comment = problem.desc_comment(conf);

        let lang = &conf.code.lang;
        let path = crate::helper::code_path(problem, Some(lang.to_owned()))?;

        if !Path::new(&path).exists() {
            let mut qr = serde_json::from_str(&problem.desc);
            if qr.is_err() {
                qr = Ok(cache
                    .get_question_silent(problem.fid, true)
                    .await
                    .map_err(|e| {
                        Error::FeatureError(format!(
                            "{:?}. name: {} id: {}",
                            e, problem.name, problem.fid,
                        ))
                    })?);
            }

            let question: Question = qr?;

            let question_desc = question.desc_comment(conf) + "\n";

            let test_path = crate::helper::test_cases_path(problem)?;

            let mut flag = false;
            let mut content = create_file_header(
                problem,
                &conf.sys.urls.problem(&problem.slug),
                &question.desc(),
            );

            for d in question.defs.0 {
                if d.value == *lang {
                    flag = true;
                    if conf.code.comment_problem_desc {
                        write!(content, "{}", p_desc_comment).unwrap();
                        write!(content, "{}", question_desc).unwrap();
                    }
                    if let Some(inject_before) = &conf.code.inject_before {
                        for line in inject_before {
                            writeln!(content, "{}", line).unwrap();
                        }
                    }
                    if conf.code.edit_code_marker {
                        writeln!(
                            content,
                            "{} {}",
                            conf.code.comment_leading.clone(),
                            &conf.code.start_marker
                        )
                        .unwrap();
                    }
                    writeln!(content, "{}", d.code).unwrap();
                    if conf.code.edit_code_marker {
                        writeln!(
                            content,
                            "{} {}",
                            conf.code.comment_leading, &conf.code.end_marker
                        )
                        .unwrap();
                    }
                    if let Some(inject_after) = &conf.code.inject_after {
                        for line in inject_after {
                            writeln!(content, "{}", line).unwrap();
                        }
                    }

                    if test_flag {
                        let mut file_tests = File::create(&test_path)?;
                        file_tests.write_all(question.all_cases.as_bytes())?;
                    }
                }
            }
            create_file_footer(&mut content);
            let content = fix_rust_code(&content)?;
            let mut file_code = File::create(&path)?;
            file_code.write_all(content.as_bytes())?;

            // if language is not found in the list of supported languges clean up files
            if !flag {
                let err_msg = format!(
                    "Question doesn't support {}, please try another. name: {}, id:{}",
                    &lang, problem.name, problem.fid
                );
                std::fs::remove_file(&path)?;
                if test_flag {
                    std::fs::remove_file(&test_path)?;
                }
                return Err(crate::Error::FeatureError(err_msg));
            }
        }

        Ok(())
    }

    fn get_percent_view(problem: &Problem) -> PathBuf {
        let index = problem.percent as usize;
        Path::new("percent").join(if index > 100 {
            "unknown"
        } else {
            PERCENT_VIEWS[index / 10]
        })
    }

    fn create_view(problem: &Problem, original: &Path) -> Result<(), Error> {
        for (relative_path, view) in [
            (
                Path::new("..").to_owned(),
                Path::new(problem.category.as_str()),
            ),
            (
                Path::new("..").to_owned(),
                Path::new(problem.display_level()),
            ),
            (
                Path::new("..").to_owned(),
                Path::new(if problem.starred {
                    "starred"
                } else {
                    "unstarred"
                }),
            ),
            (Path::new("..").join(".."), &Self::get_percent_view(problem)),
        ] {
            let view_dir = original.parent().unwrap().join(view);
            create_dir_all(&view_dir)?;
            symlink(
                relative_path.join(original.file_name().unwrap()),
                view_dir.join(original.file_name().unwrap()),
            )?
        }
        Ok(())
    }
}

#[async_trait]
impl Command for PopulateCommand {
    /// `populate` usage
    fn usage() -> ClapCommand {
        ClapCommand::new("populate")
            .about("populate question by id")
            .visible_alias("o")
            .arg(
                Arg::new("lang")
                    .short('l')
                    .long("lang")
                    .num_args(1)
                    .help("Populate with specific language"),
            )
            .arg(
                Arg::new("continue_on_error")
                    .short('c')
                    .long("continue_on_error")
                    .help("Populate with specific language")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("skip_premium")
                    .short('s')
                    .long("skip_premium")
                    .help("skip populating premium questions")
                    .action(ArgAction::SetTrue),
            )
    }

    /// `populate` handler
    async fn handler(m: &ArgMatches) -> Result<(), crate::Error> {
        term::init(false);
        term::hide_cursor()?;
        use crate::Cache;

        let mut cache = Cache::new()?;
        let mut problems = cache.get_problems()?;

        if problems.is_empty() {
            println!("downloading problems.");
            cache.download_problems().await?;
            cache = Cache::new()?;
            problems = cache.get_problems()?;
        }

        let mut conf = cache.to_owned().0.conf;

        // condition language
        if m.contains_id("lang") {
            conf.code.lang = m
                .get_one::<String>("lang")
                .ok_or(Error::NoneError)?
                .to_string();
            conf.sync()?;
        }

        let mut mod_rs_files = HashMap::new();
        let continue_on_error = m.contains_id("continue_on_error");
        let skip_premium = m.contains_id("skip_premium");

        let mut premium_count = 0;
        let mut error_count = 0;
        let mut pb = tqdm!(
            total = problems.len(),
            desc = "writing problems ",
            animation = "fillup",
            position = 0,
            force_refresh = true
        );

        for problem in &mut problems {
            let _ = pb.update(1);
            if skip_premium && problem.locked {
                premium_count += 1;
                let err_msg = format!(
                    "premium question. name: {}, id:{}",
                    problem.name, problem.fid
                );
                let _ = pb.write(format!("{} {}", "skipping".yellow(), err_msg));
                continue;
            }

            let ret = Self::write_file(problem, &conf, &cache).await;
            if ret.is_err() && continue_on_error {
                error_count += 1;
                let _ = pb.write(format!("{:?}", ret.unwrap_err()));
                continue;
            } else {
                ret?;
            }

            let module = PathBuf::from(code_path(problem, None)?);
            let mod_path = module.parent().unwrap().join("mod.rs");
            let mod_name = module.file_stem().unwrap().to_string_lossy().to_string();
            let mut mod_file = OpenOptions::new()
                .append(true)
                .create(true)
                .write(true)
                .open(&mod_path)?;
            Self::create_view(problem, &module)?;
            mod_file.write_all(format!("mod {};\n", mod_name).as_bytes())?;
            mod_rs_files.insert(mod_path, mod_file);
        }

        let mut pb = tqdm!(
            total = mod_rs_files.len(),
            desc = "writing module ",
            animation = "fillup",
            position = 1,
            force_refresh = true
        );
        for mod_rs in mod_rs_files.values_mut() {
            mod_rs.write_all("\n\n#[allow(dead_code)]\n".as_bytes())?;
            mod_rs.write_all("pub(crate) struct Solution;\n".as_bytes())?;
            let _ = pb.update(1);
        }
        drop(pb);

        println!(
            "\n\n\nproblems found: {}",
            problems.len().to_string().bright_white()
        );
        println!("premium questions: {}", premium_count.to_string().green());
        println!("errors encountered: {}", error_count.to_string().red());
        Ok(())
    }
}
