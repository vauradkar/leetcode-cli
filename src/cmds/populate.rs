//! Populate command
use std::{
    collections::HashMap,
    fs::{create_dir_all, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use super::Command;
use crate::{cache::models::Problem, helper::code_path, Cache, Config, Error};
use async_trait::async_trait;
use clap::{Arg, ArgMatches, Command as ClapCommand};
use std::os::unix::fs::symlink;

static PERCENT_VIEWS: &[&str] = &[
    "0-10", "10-20", "20-30", "30-40", "40-50", "50-60", "60-70", "70-80", "80-90", "90-100",
];

static RUST_DOC_STR_START: &str = "//! ";
static RUST_DOC_STR_END: &str = "\n//!\n";

fn write_comment(file: &mut File, comment: &str) -> Result<(), Error> {
    file.write_all(format!("{}{}{}", RUST_DOC_STR_START, comment, RUST_DOC_STR_END).as_bytes())?;
    Ok(())
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
///     -h, --help       Prints help information
///     -V, --version    Prints version information
///
/// ARGS:
///     <id>    question id
/// ```
pub struct PopulateCommand;

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
                qr = Ok(cache.get_question(problem.fid).await?);
            }

            let question: Question = qr?;

            let mut file_code = File::create(&path)?;
            let question_desc = question.desc_comment(conf) + "\n";

            let test_path = crate::helper::test_cases_path(problem)?;

            let mut flag = false;

            write_comment(&mut file_code, "# Challenge info")?;
            write_comment(
                &mut file_code,
                &format!("url: <{}>", conf.sys.urls.problem(&problem.slug)),
            )?;
            write_comment(&mut file_code, &format!("percent: {}", problem.percent))?;
            write_comment(
                &mut file_code,
                &format!("level: {}", problem.display_level()),
            )?;
            write_comment(&mut file_code, &format!("category: {}", problem.category))?;

            write_comment(&mut file_code, "# Question")?;
            for q_line in question.desc().lines() {
                write_comment(&mut file_code, q_line)?;
            }
            file_code.write_all("use crate::solutions::Solution;\n\n".as_bytes())?;
            for d in question.defs.0 {
                if d.value == *lang {
                    flag = true;
                    if conf.code.comment_problem_desc {
                        file_code.write_all(p_desc_comment.as_bytes())?;
                        file_code.write_all(question_desc.as_bytes())?;
                    }
                    if let Some(inject_before) = &conf.code.inject_before {
                        for line in inject_before {
                            file_code.write_all((line.to_string() + "\n").as_bytes())?;
                        }
                    }
                    if conf.code.edit_code_marker {
                        file_code.write_all(
                            (conf.code.comment_leading.clone()
                                + " "
                                + &conf.code.start_marker
                                + "\n")
                                .as_bytes(),
                        )?;
                    }
                    file_code.write_all((d.code.to_string() + "\n").as_bytes())?;
                    if conf.code.edit_code_marker {
                        file_code.write_all(
                            (conf.code.comment_leading.clone()
                                + " "
                                + &conf.code.end_marker
                                + "\n")
                                .as_bytes(),
                        )?;
                    }
                    if let Some(inject_after) = &conf.code.inject_after {
                        for line in inject_after {
                            file_code.write_all((line.to_string() + "\n").as_bytes())?;
                        }
                    }

                    if test_flag {
                        let mut file_tests = File::create(&test_path)?;
                        file_tests.write_all(question.all_cases.as_bytes())?;
                    }
                }
            }

            // if language is not found in the list of supported languges clean up files
            if !flag {
                std::fs::remove_file(&path)?;
                if test_flag {
                    std::fs::remove_file(&test_path)?;
                }
                return Err(crate::Error::FeatureError(format!(
                    "This question doesn't support {}, please try another",
                    &lang
                )));
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
        for view in [
            Path::new(problem.category.as_str()),
            Path::new(problem.display_level()),
            Path::new(if problem.starred {
                "starred"
            } else {
                "unstarred"
            }),
            &Self::get_percent_view(problem),
        ] {
            let view_dir = original.parent().unwrap().join(view);
            create_dir_all(&view_dir)?;
            symlink(original, view_dir.join(original.file_name().unwrap()))?
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
    }

    /// `populate` handler
    async fn handler(m: &ArgMatches) -> Result<(), crate::Error> {
        use crate::Cache;

        let mut cache = Cache::new()?;
        let mut problems = cache.get_problems()?;

        if problems.is_empty() {
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

        for problem in &mut problems[..10] {
            Self::write_file(problem, &conf, &cache).await?;
            let module = PathBuf::from(code_path(problem, None)?);
            let mod_path = module.parent().unwrap().join("mod.rs");
            let mod_name = module.file_stem().unwrap().to_string_lossy().to_string();
            let mut mod_file = OpenOptions::new()
                .append(true)
                .create(true)
                .write(true)
                .open(&mod_path)?;
            Self::create_view(problem, &module)?;
            mod_file.write_all(format!("// mod {};\n", mod_name).as_bytes())?;
            mod_rs_files.insert(mod_path, mod_file);
        }

        for mod_rs in mod_rs_files.values_mut() {
            mod_rs.write_all("\n\npub(crate) struct Solution;\n".as_bytes())?;
        }
        Ok(())
    }
}
