//! `jx import` for Gradle builds.
//!
//! Gradle scripts are code, not data. This module therefore reads only the
//! declarative subset jx can translate faithfully: `dependencies` blocks
//! (string or map notation, `platform(...)` imports and `constraints`),
//! `java` toolchain settings and `application` main classes. Anything that
//! needs evaluation — variables, string interpolation, `project(...)`,
//! `files(...)`, version catalogs, dependency blocks wrapped in control flow
//! — stops the import with an explanation instead of producing a manifest
//! that silently differs from the Gradle build.
//!
//! Versionless dependencies are accepted only when the same `dependencies`
//! block imports a platform BOM; jx's `[dependencies]` can express that, and
//! `jx lock` will then verify that the BOM really manages the artifact.

use std::collections::HashMap;

use jx_common::error::{JxError, Kind};

use crate::{ImportedManifest, detect_framework, parse_java_version, push_dependency, toml_string};

/// One Gradle build to import.
pub struct GradleSource {
    /// Display path of the build script (for diagnostics).
    pub script: String,
    pub text: String,
    /// `settings.gradle.kts` (or `settings.gradle`): display path and text.
    pub settings: Option<(String, String)>,
    /// `gradle.properties`: display path and text.
    pub properties: Option<(String, String)>,
    /// Directory name, used when the build declares no project name.
    pub dir_name: String,
    /// Version to use when neither the build nor gradle.properties sets one.
    pub default_version: String,
    /// JDK to use when the build declares no toolchain or compatibility.
    pub default_feature: u32,
}

/// One dependency on its way into the manifest. `version` is `None` only for
/// a dependency managed by a platform imported in the same table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dep {
    group_id: String,
    artifact_id: String,
    version: Option<String>,
    classifier: Option<String>,
    platform: bool,
    line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str { text: String, interpolated: bool },
    Num(String),
    Punct(char),
    Newline,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    line: usize,
}

impl Token {
    fn ident(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident(name) => Some(name),
            _ => None,
        }
    }

    fn string(&self) -> Option<(&str, bool)> {
        match &self.tok {
            Tok::Str { text, interpolated } => Some((text, *interpolated)),
            _ => None,
        }
    }
}

struct Block<'a> {
    name: String,
    body: &'a [Token],
    line: usize,
}

struct Parsed {
    name: Option<String>,
    version: Option<String>,
    version_source: Option<String>,
    group: Option<String>,
    jdk: Option<(u32, String)>,
    main_class: Option<String>,
    main: Vec<Dep>,
    dev: Vec<Dep>,
    overrides: Vec<Dep>,
    plugins: Vec<String>,
    /// Version of `org.springframework.boot`, when the build applies it.
    spring_boot: Option<String>,
    repositories: bool,
    buildscript: bool,
    /// Declarations jx cannot import: `kapt`/`ksp`, the buildscript
    /// classpath, and Java processors without a literal version.
    processors: usize,
    /// Java processors with a literal version, bound for `[processors]`.
    processor_deps: Vec<Dep>,
    compile_only: usize,
    runtime_only: usize,
    test_compile_only: usize,
    configuration_blocks: usize,
    constraints: usize,
    unknown_configs: Vec<(String, usize)>,
    multi_module: bool,
    /// Literal `ext`/`val` assignments, the only variables a static import
    /// can evaluate.
    vars: HashMap<String, String>,
}

/// Convert a Gradle build script into a jx manifest.
pub fn manifest_from_gradle(source: &GradleSource) -> Result<ImportedManifest, JxError> {
    let tokens = lex(&source.script, &source.text)?;
    let mut parsed = Parsed {
        name: None,
        version: None,
        version_source: None,
        group: None,
        jdk: None,
        main_class: None,
        main: Vec::new(),
        dev: Vec::new(),
        overrides: Vec::new(),
        plugins: Vec::new(),
        spring_boot: None,
        repositories: false,
        buildscript: false,
        processors: 0,
        processor_deps: Vec::new(),
        compile_only: 0,
        runtime_only: 0,
        test_compile_only: 0,
        configuration_blocks: 0,
        constraints: 0,
        unknown_configs: Vec::new(),
        multi_module: false,
        vars: HashMap::new(),
    };

    // Literal variables must be known before the dependencies block is read;
    // collect them first, then the blocks that consume them.
    parse_top_level_assignments(&source.script, &tokens, &mut parsed)?;

    for block in top_level_blocks(&tokens) {
        match block.name.as_str() {
            "dependencies" => parse_dependencies(&source.script, block.body, &mut parsed, false)?,
            "plugins" => collect_plugins(block.body, &mut parsed),
            "repositories" => parsed.repositories = true,
            "buildscript" => parsed.buildscript = true,
            "java" => {
                if parsed.jdk.is_none()
                    && let Some(found) = detect_jdk(block.body)
                {
                    parsed.jdk = Some(found);
                }
            }
            "application" => {
                if parsed.main_class.is_none() {
                    parsed.main_class = detect_main_class(block.body);
                    if parsed.main_class.is_none()
                        && block
                            .body
                            .iter()
                            .any(|token| token.ident() == Some("mainClass"))
                    {
                        return Err(error(
                            &source.script,
                            block.line,
                            "the application mainClass is not a literal string",
                        )
                        .hint(
                            "jx import cannot evaluate it; write mainClass = \"com.example.App\" \
                             or pin `[run] main` after the import",
                        ));
                    }
                }
            }
            "allprojects" | "subprojects"
                if top_level_blocks(block.body)
                    .iter()
                    .any(|nested| nested.name == "dependencies") =>
            {
                return Err(error(
                    &source.script,
                    block.line,
                    format!(
                        "`{}` declares dependencies for this project and its children",
                        block.name
                    ),
                )
                .hint(
                    "jx imports one module; run `jx import` inside the application module \
                     directory, or move those declarations into that module's build script",
                ));
            }
            _ => {}
        }
    }

    // Name: settings.gradle(.kts) first, then the directory name.
    let mut notes: Vec<String> = Vec::new();
    if let Some((settings_path, settings_text)) = &source.settings {
        let settings_tokens = lex(settings_path, settings_text)?;
        parsed.name = settings_root_name(&settings_tokens);
        if settings_includes(&settings_tokens) {
            parsed.multi_module = true;
        }
    }
    let name_fallback = parsed.name.is_none();
    let name = parsed
        .name
        .clone()
        .unwrap_or_else(|| source.dir_name.clone());

    // Version: build script, then gradle.properties, then the default.
    if parsed.version.is_none()
        && let Some((properties_path, properties_text)) = &source.properties
        && let Some(version) = property(properties_text, "version")
    {
        parsed.version = Some(version);
        parsed.version_source = Some(format!("{properties_path} version"));
    }
    let version_fallback = parsed.version.is_none();
    let version = parsed
        .version
        .clone()
        .unwrap_or_else(|| source.default_version.clone());
    if parsed.group.is_none()
        && let Some((_, properties_text)) = &source.properties
    {
        parsed.group = property(properties_text, "group");
    }

    if name_fallback {
        notes.push(format!(
            "no rootProject.name in the Gradle settings; using the directory name {name:?}"
        ));
    }
    if version_fallback {
        notes.push(format!(
            "no version in the Gradle build or gradle.properties; using version = {version:?}"
        ));
    }
    if let Some(source_name) = &parsed.version_source {
        notes.push(format!("version from {source_name}"));
    }
    if let Some(group) = &parsed.group {
        notes.push(format!(
            "group {group:?} has no project.toml equivalent; jx derives coordinates at export time"
        ));
    }

    let (jdk, jdk_source) = match parsed.jdk.clone() {
        Some((feature, found)) => {
            notes.push(format!("jdk = \"{feature}\" from the Gradle {found}"));
            (feature, Some(found))
        }
        None => {
            notes.push(format!(
                "no Java toolchain or compatibility in the Gradle build; using jdk = \"{}\" (the jx default)",
                source.default_feature
            ));
            (source.default_feature, None)
        }
    };
    if !(8..=99).contains(&jdk) {
        return Err(error(
            &source.script,
            1,
            format!("the Gradle build asks for Java {jdk}, which jx cannot pin"),
        )
        .hint("V1 pins JDK feature releases between 8 and 99"));
    }

    // A versionless dependency is only reproducible when a platform BOM
    // supplies its version. The Spring Boot plugin applies the Boot BOM
    // itself, so that documented case becomes a real platform entry; nothing
    // else is inferred.
    let has_platform = parsed.main.iter().any(|dep| dep.platform);
    let versionless = parsed
        .main
        .iter()
        .chain(&parsed.dev)
        .find(|dep| dep.version.is_none() && !dep.platform);
    let mut boot_platform = None;
    if let Some(dep) = versionless {
        if has_platform {
            // A platform import already supplies versions; jx lock verifies it.
        } else if let Some(boot) = &parsed.spring_boot {
            parsed.main.insert(
                0,
                Dep {
                    group_id: "org.springframework.boot".into(),
                    artifact_id: "spring-boot-dependencies".into(),
                    version: Some(boot.clone()),
                    classifier: None,
                    platform: true,
                    line: dep.line,
                },
            );
            boot_platform = Some(boot.clone());
        } else {
            return Err(error(
                &source.script,
                dep.line,
                format!(
                    "{}:{} has no version, and no platform BOM is imported",
                    dep.group_id, dep.artifact_id
                ),
            )
            .hint(
                "add an explicit version, or add \
                 implementation(platform(\"group:bom:version\")) to the Gradle build, then import again",
            ));
        }
    }

    // A manifest table may name one group:artifact only once.
    check_duplicates(&source.script, &parsed.main)?;
    check_duplicates(&source.script, &parsed.dev)?;
    check_duplicates(&source.script, &parsed.processor_deps)?;
    let mut dev = parsed.dev.clone();
    let before = dev.len();
    dev.retain(|dep| {
        !parsed.main.iter().any(|main| {
            main.group_id == dep.group_id
                && main.artifact_id == dep.artifact_id
                && main.classifier == dep.classifier
        })
    });
    if dev.len() != before {
        notes.push(format!(
            "{} test dependenc{} also declared as a main dependency; kept in [dependencies]",
            before - dev.len(),
            if before - dev.len() == 1 {
                "y is"
            } else {
                "ies are"
            }
        ));
    }

    if !parsed.plugins.is_empty() {
        notes.push(format!(
            "{} Gradle plugin(s) are not migrated ({}); only dependency, toolchain and main-class declarations were read",
            parsed.plugins.len(),
            parsed.plugins.join(", ")
        ));
    }
    if parsed
        .plugins
        .iter()
        .any(|plugin| plugin.contains("spring") || plugin.contains("quarkus"))
    {
        notes.push(
            "an application framework plugin was found; its executable-jar/repackage behaviour \
             is not reproduced, and the jx project produces a plain jar"
                .to_string(),
        );
    }
    if parsed.repositories {
        notes.push(
            "Gradle repositories are not migrated; jx only trusts repositories configured in config.toml"
                .to_string(),
        );
    }
    if parsed.buildscript {
        notes.push(
            "the buildscript classpath is not part of the application and was not imported"
                .to_string(),
        );
    }
    if !parsed.processor_deps.is_empty() {
        notes.push(format!(
            "{} annotation processor(s) imported into [processors] with the declared version; jx runs them through javac -processorpath for main and test compilation",
            parsed.processor_deps.len()
        ));
    }
    if parsed.processors > 0 {
        notes.push(format!(
            "{} annotation-processor declaration(s) were not imported (kapt/ksp, the buildscript classpath, or no literal version); add them to [processors] by hand",
            parsed.processors
        ));
    }
    if parsed.compile_only > 0 {
        notes.push(format!(
            "{} compileOnly dependenc{} imported into [dependencies]; jx has no provided scope and will also package {}",
            parsed.compile_only,
            if parsed.compile_only == 1 { "y was" } else { "ies were" },
            if parsed.compile_only == 1 { "it" } else { "them" }
        ));
    }
    if parsed.runtime_only > 0 {
        notes.push(format!(
            "{} runtimeOnly dependenc{} imported into [dependencies]; jx has no separate runtime scope",
            parsed.runtime_only,
            if parsed.runtime_only == 1 { "y was" } else { "ies were" }
        ));
    }
    if parsed.test_compile_only > 0 {
        notes.push(format!(
            "{} testCompileOnly dependenc{} imported into [dev-dependencies]",
            parsed.test_compile_only,
            if parsed.test_compile_only == 1 {
                "y was"
            } else {
                "ies were"
            }
        ));
    }
    if !parsed.unknown_configs.is_empty() {
        let shown: Vec<String> = parsed
            .unknown_configs
            .iter()
            .map(|(name, count)| format!("{name} ({count})"))
            .collect();
        notes.push(format!(
            "skipped configuration(s) jx cannot represent: {}",
            shown.join(", ")
        ));
    }
    if parsed.configuration_blocks > 0 {
        notes.push(format!(
            "{} dependency configuration block(s) (exclude/transitive/artifacts) were not imported; jx's graph may be larger",
            parsed.configuration_blocks
        ));
    }
    if parsed.constraints > 0 {
        notes.push(format!(
            "{} Gradle constraint(s) became [overrides]; jx overrides apply to the whole graph, not one configuration",
            parsed.constraints
        ));
    }
    let platform_count = parsed.main.iter().filter(|dep| dep.platform).count();
    if platform_count > 0 {
        notes.push(format!(
            "{platform_count} platform BOM import(s) were written as platform entries; `jx lock` verifies that they manage every versionless dependency"
        ));
    }
    if let Some(boot) = &boot_platform {
        notes.push(format!(
            "versionless dependencies were resolved through the Spring Boot plugin; imported org.springframework.boot:spring-boot-dependencies:{boot} as a platform BOM. `jx lock` names any artifact that BOM does not manage"
        ));
    }
    if parsed.multi_module {
        notes.push(
            "the Gradle settings file includes other projects; only this module was imported"
                .to_string(),
        );
    }
    if !dev.is_empty() {
        notes.push(
            "test dependencies were imported, but `jx test` runs JUnit 5 through the JUnit Platform console standalone; if the build does not declare it, add it with `jx add --dev org.junit.platform:junit-platform-console-standalone`"
                .to_string(),
        );
    }
    notes.push(
        "direct dependencies are exact-pinned with `=`; transitive versions still follow jx's nearest-wins resolver"
            .to_string(),
    );

    // A Spring Boot platform entry inserted above already carries the
    // org.springframework.boot group, so group detection covers the Boot
    // plugin without a special case.
    let framework = detect_framework(
        parsed
            .main
            .iter()
            .chain(&parsed.processor_deps)
            .map(|dep| dep.group_id.as_str()),
    );
    if let Some((value, group)) = framework {
        notes.push(format!(
            "detected framework = \"{value}\" from the {group} group; remove [package] framework if this module is a plain jar"
        ));
    }
    let text = render_gradle(GradleRender {
        header: ManifestHeader {
            name: &name,
            version: &version,
            jdk,
        },
        main: &parsed.main,
        dev: &dev,
        processors: &parsed.processor_deps,
        overrides: &parsed.overrides,
        main_class: parsed.main_class.as_deref(),
        framework: framework.map(|(value, _)| value),
        source: &source.script,
    });
    Ok(ImportedManifest {
        text,
        notes,
        dependencies: parsed.main.iter().filter(|dep| !dep.platform).count(),
        dev_dependencies: dev.len(),
        processors: parsed.processor_deps.len(),
        jdk_source,
    })
}

fn error(source: &str, line: usize, message: impl Into<String>) -> JxError {
    JxError::new(Kind::Resolve, message).at(format!("{source}:{line}"))
}

fn lex(source: &str, text: &str) -> Result<Vec<Token>, JxError> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    let mut line = 1usize;
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                tokens.push(Token {
                    tok: Tok::Newline,
                    line,
                });
                line += 1;
            }
            '\r' | ' ' | '\t' => {}
            '/' => match chars.peek() {
                Some('/') => {
                    while let Some(&next) = chars.peek() {
                        if next == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    let mut closed = false;
                    for next in chars.by_ref() {
                        if next == '\n' {
                            line += 1;
                        }
                        if previous == '*' && next == '/' {
                            closed = true;
                            break;
                        }
                        previous = next;
                    }
                    if !closed {
                        return Err(error(source, line, "unterminated /* comment */"));
                    }
                }
                _ => tokens.push(Token {
                    tok: Tok::Punct('/'),
                    line,
                }),
            },
            '\'' | '"' => {
                let start_line = line;
                let quote = c;
                let mut value = String::new();
                let mut interpolated = false;
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == '\\' {
                        if let Some(escaped) = chars.next() {
                            if escaped == '\n' {
                                line += 1;
                            }
                            value.push(escaped);
                            continue;
                        }
                        break;
                    }
                    if next == quote {
                        closed = true;
                        break;
                    }
                    if next == '\n' {
                        line += 1;
                    }
                    if quote == '"' && next == '$' {
                        interpolated = true;
                    }
                    value.push(next);
                }
                if !closed {
                    return Err(error(
                        source,
                        start_line,
                        "unterminated string in the Gradle build",
                    ));
                }
                tokens.push(Token {
                    tok: Tok::Str {
                        text: value,
                        interpolated,
                    },
                    line: start_line,
                });
            }
            c if c.is_ascii_alphanumeric() || c == '_' => {
                let mut ident = String::new();
                ident.push(c);
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_alphanumeric() || next == '_' {
                        ident.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let tok = if ident.chars().all(|c| c.is_ascii_digit()) {
                    Tok::Num(ident)
                } else {
                    Tok::Ident(ident)
                };
                tokens.push(Token { tok, line });
            }
            other => tokens.push(Token {
                tok: Tok::Punct(other),
                line,
            }),
        }
    }
    Ok(tokens)
}

/// Blocks written as `name { ... }` at the top level of `tokens`. Unknown
/// blocks are skipped whole so their `dependencies` never look top-level.
fn top_level_blocks(tokens: &[Token]) -> Vec<Block<'_>> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let Some(name) = tokens[i].ident() {
            let next = next_significant(tokens, i + 1);
            if let Some(open) = next
                && matches!(tokens[open].tok, Tok::Punct('{'))
                && let Some((start, end)) = block_at(tokens, open)
            {
                blocks.push(Block {
                    name: name.to_string(),
                    body: &tokens[start..end],
                    line: tokens[i].line,
                });
                i = end + 1;
                continue;
            }
        }
        if matches!(tokens[i].tok, Tok::Punct('{'))
            && let Some((_, end)) = block_at(tokens, i)
        {
            i = end + 1;
            continue;
        }
        i += 1;
    }
    blocks
}

fn next_significant(tokens: &[Token], from: usize) -> Option<usize> {
    (from..tokens.len()).find(|&i| !matches!(tokens[i].tok, Tok::Newline))
}

/// Body range of the block whose `{` is at `open` (exclusive end).
fn block_at(tokens: &[Token], open: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut i = open;
    while i < tokens.len() {
        match tokens[i].tok {
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((open + 1, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn statements(tokens: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    for (i, token) in tokens.iter().enumerate() {
        match token.tok {
            Tok::Punct('{') | Tok::Punct('(') | Tok::Punct('[') => depth += 1,
            Tok::Punct('}') | Tok::Punct(')') | Tok::Punct(']') => depth = (depth - 1).max(0),
            Tok::Newline | Tok::Punct(';') if depth == 0 => {
                if i > start {
                    out.push(&tokens[start..i]);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < tokens.len() {
        out.push(&tokens[start..]);
    }
    out
}

fn significant(tokens: &[Token]) -> Vec<&Token> {
    tokens
        .iter()
        .filter(|token| !matches!(token.tok, Tok::Newline))
        .collect()
}

fn parse_dependencies(
    source: &str,
    body: &[Token],
    parsed: &mut Parsed,
    constraints: bool,
) -> Result<(), JxError> {
    for statement in statements(body) {
        let sig = significant(statement);
        let Some(head) = sig.first().and_then(|token| token.ident()) else {
            continue;
        };
        if matches!(
            head,
            "if" | "else" | "for" | "while" | "when" | "switch" | "each" | "forEach"
        ) {
            return Err(error(
                source,
                statement[0].line,
                format!("`{head}` inside a dependencies block cannot be imported statically"),
            )
            .hint(
                "jx import reads declarations only; keep dependency declarations unconditional, \
                 or keep this module on Gradle",
            ));
        }
        if head == "constraints" {
            if let Some(open) = statement
                .iter()
                .position(|token| matches!(token.tok, Tok::Punct('{')))
                && let Some((start, end)) = block_at(statement, open)
            {
                parse_dependencies(source, &statement[start..end], parsed, true)?;
            }
            continue;
        }

        let vars = parsed.vars.clone();
        let scope = match head {
            "implementation" | "api" => Scope::Main,
            "compileOnly" | "compileOnlyApi" => Scope::CompileOnly,
            "runtimeOnly" => Scope::RuntimeOnly,
            "testImplementation" => Scope::Dev,
            "testCompileOnly" => Scope::TestCompileOnly,
            "testRuntimeOnly" => Scope::Dev,
            "annotationProcessor" | "testAnnotationProcessor" | "kapt" | "ksp" | "classpath" => {
                Scope::Processor
            }
            _ => Scope::Unknown,
        };

        let Some((args, has_block)) = call_arguments(source, statement, &sig, head)? else {
            continue;
        };
        if has_block {
            parsed.configuration_blocks += 1;
        }
        if scope == Scope::Unknown {
            match parsed
                .unknown_configs
                .iter_mut()
                .find(|(name, _)| name == head)
            {
                Some((_, count)) => *count += 1,
                None => parsed.unknown_configs.push((head.to_string(), 1)),
            }
            continue;
        }
        if scope == Scope::Processor {
            // Kotlin and buildscript processors stay outside jx's Java build;
            // a Java processor is importable only with a literal version.
            let importable = matches!(head, "annotationProcessor" | "testAnnotationProcessor");
            let dep = importable
                .then(|| parse_notation(source, &args, statement[0].line, &vars))
                .and_then(Result::ok)
                .map(Notation::into_dep)
                .filter(|dep| dep.version.is_some() && !dep.platform);
            match dep {
                Some(dep) if !same_processor(&parsed.processor_deps, &dep) => {
                    parsed.processor_deps.push(dep);
                }
                // The same coordinate declared for main and test compilation.
                Some(_) => {}
                None => parsed.processors += 1,
            }
            continue;
        }

        let notation = parse_notation(source, &args, statement[0].line, &vars)?;
        match scope {
            Scope::Processor | Scope::Unknown => {}
            _ => {
                let dep = notation.into_dep();
                if constraints {
                    if dep.platform {
                        return Err(error(
                            source,
                            dep.line,
                            "a platform constraint cannot become a jx override",
                        )
                        .hint("import the BOM as a platform in the dependencies block instead"));
                    }
                    if dep.version.is_none() {
                        return Err(error(
                            source,
                            dep.line,
                            format!(
                                "{}:{} has no constraint version",
                                dep.group_id, dep.artifact_id
                            ),
                        ));
                    }
                    parsed.constraints += 1;
                    parsed.overrides.push(dep);
                    continue;
                }
                if dep.platform && dep.version.is_none() {
                    return Err(error(
                        source,
                        dep.line,
                        "a platform import needs a BOM version",
                    ));
                }
                if !dev_scope(scope) && !dep.platform {
                    match scope {
                        Scope::CompileOnly => parsed.compile_only += 1,
                        Scope::RuntimeOnly => parsed.runtime_only += 1,
                        _ => {}
                    }
                }
                if scope == Scope::TestCompileOnly {
                    parsed.test_compile_only += 1;
                }
                if dep.platform {
                    // Platform imports are table-wide in jx; keep them in
                    // [dependencies] even when Gradle declared them for tests.
                    parsed.main.push(dep);
                } else if dev_scope(scope) {
                    parsed.dev.push(dep);
                } else {
                    parsed.main.push(dep);
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Main,
    Dev,
    CompileOnly,
    RuntimeOnly,
    TestCompileOnly,
    Processor,
    Unknown,
}

fn dev_scope(scope: Scope) -> bool {
    matches!(scope, Scope::Dev | Scope::TestCompileOnly)
}

/// Arguments of a configuration call, with the optional trailing
/// configuration block noted. Returns `None` for a statement that is not a
/// dependency declaration (for example a bare property assignment).
fn call_arguments(
    source: &str,
    statement: &[Token],
    sig: &[&Token],
    head: &str,
) -> Result<Option<(Vec<Token>, bool)>, JxError> {
    let Some(second) = sig.get(1) else {
        return Ok(None);
    };
    match second.tok {
        Tok::Punct('(') => {
            let mut depth = 0i32;
            let mut close = None;
            for (i, token) in sig.iter().enumerate().skip(1) {
                match token.tok {
                    Tok::Punct('(') => depth += 1,
                    Tok::Punct(')') => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(close) = close else {
                return Err(error(
                    source,
                    statement[0].line,
                    format!("`{head}` has an unterminated argument list"),
                ));
            };
            let args = sig[2..close].iter().map(|token| (*token).clone()).collect();
            let has_block = sig
                .iter()
                .skip(close + 1)
                .any(|token| matches!(token.tok, Tok::Punct('{')));
            Ok(Some((args, has_block)))
        }
        Tok::Str { .. } => Ok(Some((
            vec![(*second).clone()],
            sig.iter()
                .skip(2)
                .any(|token| matches!(token.tok, Tok::Punct('{'))),
        ))),
        Tok::Ident(_) => Ok(Some((
            sig[1..].iter().map(|token| (*token).clone()).collect(),
            sig.iter()
                .skip(1)
                .any(|token| matches!(token.tok, Tok::Punct('{'))),
        ))),
        _ => Ok(None),
    }
}

enum Notation {
    Plain(Dep),
    Platform(Dep),
}

impl Notation {
    fn into_dep(self) -> Dep {
        match self {
            Notation::Plain(dep) | Notation::Platform(dep) => dep,
        }
    }
}

fn parse_notation(
    source: &str,
    args: &[Token],
    line: usize,
    vars: &HashMap<String, String>,
) -> Result<Notation, JxError> {
    let args: Vec<&Token> = args
        .iter()
        .filter(|token| !matches!(token.tok, Tok::Newline))
        .collect();
    let Some(first) = args.first() else {
        return Err(error(source, line, "an empty dependency declaration"));
    };
    if let Some((raw, interpolated)) = first.string() {
        let raw = if interpolated {
            substitute(raw, vars).ok_or_else(|| dynamic_string(source, line, raw))?
        } else {
            raw.to_string()
        };
        let dep = dep_from_notation(source, &raw, line, false)?;
        return Ok(Notation::Plain(dep));
    }
    let Some(name) = first.ident() else {
        return Err(error(
            source,
            line,
            "the dependency argument is not a string literal",
        )
        .hint("use `group:artifact:version` or a map of group/name/version"));
    };
    match name {
        "platform" | "enforcedPlatform" => {
            let raw = only_string_argument(source, &args, line, name, vars)?;
            let dep = dep_from_notation(source, &raw, line, true)?;
            Ok(Notation::Platform(dep))
        }
        "project" | "files" | "fileTree" | "libs" | "gradleApi" | "localGroovy" => Err(error(
            source,
            line,
            format!("`{name}(...)` is a build-local dependency jx cannot resolve"),
        )
        .hint(
            "publish that code as an artifact, or vendor it into src/main/java; \
             version catalogs (libs.*) must be replaced with literal coordinates",
        )),
        "group" => {
            let dep = map_notation(source, &args, line, vars)?;
            Ok(Notation::Plain(dep))
        }
        other => Err(error(
            source,
            line,
            format!("could not read the dependency argument `{other}`"),
        )
        .hint(
            "jx import accepts a literal coordinate or a platform(...) import; \
             variables and version catalogs require evaluating the Gradle build",
        )),
    }
}

fn only_string_argument(
    source: &str,
    args: &[&Token],
    line: usize,
    name: &str,
    vars: &HashMap<String, String>,
) -> Result<String, JxError> {
    // platform("g:a:v") or platform 'g:a:v' (Groovy)
    let rest: Vec<&&Token> = args
        .iter()
        .skip(1)
        .filter(|token| !matches!(token.tok, Tok::Punct('(') | Tok::Punct(')')))
        .collect();
    match rest.as_slice() {
        [token] => {
            let Some((raw, interpolated)) = token.string() else {
                return Err(error(
                    source,
                    line,
                    format!("`{name}(...)` needs a literal coordinate"),
                ));
            };
            if interpolated {
                return substitute(raw, vars).ok_or_else(|| dynamic_string(source, line, raw));
            }
            Ok(raw.to_string())
        }
        _ => Err(error(
            source,
            line,
            format!("`{name}(...)` needs one literal coordinate"),
        )),
    }
}

fn map_notation(
    source: &str,
    args: &[&Token],
    line: usize,
    vars: &HashMap<String, String>,
) -> Result<Dep, JxError> {
    let mut group = None;
    let mut artifact = None;
    let mut version = None;
    let mut classifier = None;
    let mut i = 0;
    while i < args.len() {
        let Some(key) = args[i].ident() else {
            i += 1;
            continue;
        };
        if i + 2 >= args.len() || !matches!(args[i + 1].tok, Tok::Punct(':')) {
            return Err(error(
                source,
                line,
                format!("could not read the map dependency key `{key}`"),
            ));
        }
        let value = match &args[i + 2].tok {
            Tok::Str { text, interpolated } => {
                if *interpolated {
                    substitute(text, vars).ok_or_else(|| dynamic_string(source, line, text))?
                } else {
                    text.clone()
                }
            }
            Tok::Num(number) => number.clone(),
            _ => {
                return Err(error(
                    source,
                    line,
                    format!("`{key}` in a map dependency is not a literal"),
                ));
            }
        };
        match key {
            "group" => group = Some(value),
            "name" => artifact = Some(value),
            "version" => version = Some(value),
            "classifier" => classifier = Some(value),
            other => {
                return Err(error(
                    source,
                    line,
                    format!("unsupported map dependency key `{other}`"),
                )
                .hint("jx understands group, name, version and classifier"));
            }
        }
        i += 3;
        while i < args.len() && matches!(args[i].tok, Tok::Punct(',')) {
            i += 1;
        }
    }
    let group = group.ok_or_else(|| error(source, line, "a map dependency needs `group:`"))?;
    let artifact = artifact.ok_or_else(|| error(source, line, "a map dependency needs `name:`"))?;
    let version = version.ok_or_else(|| {
        error(
            source,
            line,
            format!("{group}:{artifact} has no `version:` in its map"),
        )
    })?;
    let mut raw = format!("{group}:{artifact}:{version}");
    if let Some(classifier) = classifier {
        raw.push(':');
        raw.push_str(&classifier);
    }
    dep_from_notation(source, &raw, line, false)
}

fn dep_from_notation(source: &str, raw: &str, line: usize, platform: bool) -> Result<Dep, JxError> {
    if raw.contains('@') {
        return Err(error(
            source,
            line,
            format!("{raw:?} selects an artifact extension with `@`, which jx does not model"),
        )
        .hint("dependencies are jars; remove the @extension"));
    }
    if raw.contains('$') {
        return Err(dynamic_string(source, line, raw));
    }
    let parts: Vec<&str> = raw.split(':').collect();
    if !(2..=4).contains(&parts.len()) {
        return Err(
            error(source, line, format!("not a Maven coordinate: {raw:?}"))
                .hint("`group:artifact[:version][:classifier]`"),
        );
    }
    let group_id = gradle_ident(source, line, "group", parts[0])?;
    let artifact_id = gradle_ident(source, line, "artifact", parts[1])?;
    let version = match parts.get(2) {
        None => None,
        Some(&"") => {
            return Err(error(source, line, format!("{raw:?} has an empty version")));
        }
        Some(text) => Some(gradle_version(source, line, text)?),
    };
    let classifier = match parts.get(3) {
        None => None,
        Some(text) => Some(gradle_ident(source, line, "classifier", text)?),
    };
    if platform && classifier.is_some() {
        return Err(error(
            source,
            line,
            "a platform import cannot have a classifier",
        ));
    }
    if classifier.is_some() && version.is_none() {
        return Err(error(
            source,
            line,
            format!("{raw:?} has a classifier but no version"),
        )
        .hint("jx needs `group:artifact:version:classifier`"));
    }
    Ok(Dep {
        group_id,
        artifact_id,
        version,
        classifier,
        platform,
        line,
    })
}

fn gradle_ident(source: &str, line: usize, what: &str, value: &str) -> Result<String, JxError> {
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(value.to_string())
    } else {
        Err(error(
            source,
            line,
            format!("the dependency {what} {value:?} is not a valid Maven identifier"),
        )
        .hint("use letters, digits and `.`, `_`, `-`"))
    }
}

fn gradle_version(source: &str, line: usize, value: &str) -> Result<String, JxError> {
    let dynamic = value == "latest.release"
        || value == "latest.integration"
        || value.contains('+')
        || value.contains('[')
        || value.contains('(')
        || value.contains(',')
        || value.contains('$');
    if dynamic {
        return Err(error(
            source,
            line,
            format!("the dependency version {value:?} is dynamic or a range"),
        )
        .hint(
            "Gradle resolves it at build time; jx pins releases, so write the version \
             the build actually selects",
        ));
    }
    if value.to_ascii_lowercase().ends_with("-snapshot") {
        return Err(error(
            source,
            line,
            format!("the dependency uses the SNAPSHOT version {value}"),
        )
        .hint("jx's lockfile pins release bytes; use a released version"));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
    {
        return Err(error(
            source,
            line,
            format!("the dependency version {value:?} contains unsupported characters"),
        )
        .hint("versions may contain letters, digits and `.`, `_`, `-`, `+`"));
    }
    Ok(value.to_string())
}

fn dynamic_string(source: &str, line: usize, raw: &str) -> JxError {
    error(
        source,
        line,
        format!("the dependency string {raw:?} interpolates a Gradle value"),
    )
    .hint(
        "jx import reads literals only; replace the variable with the resolved coordinate, \
         or keep this module on Gradle",
    )
}

/// Whether the same coordinate is already queued as a processor. Gradle
/// commonly declares one processor for both main and test compilation.
fn same_processor(existing: &[Dep], dep: &Dep) -> bool {
    existing.iter().any(|other| {
        other.group_id == dep.group_id
            && other.artifact_id == dep.artifact_id
            && other.classifier == dep.classifier
            && other.version == dep.version
    })
}

fn check_duplicates(source: &str, deps: &[Dep]) -> Result<(), JxError> {
    for (index, dep) in deps.iter().enumerate() {
        if deps[..index].iter().any(|earlier| {
            earlier.group_id == dep.group_id
                && earlier.artifact_id == dep.artifact_id
                && earlier.classifier == dep.classifier
        }) {
            return Err(error(
                source,
                dep.line,
                format!(
                    "{}:{} is declared more than once",
                    dep.group_id, dep.artifact_id
                ),
            )
            .hint("project.toml allows one entry per group:artifact (and classifier)"));
        }
    }
    Ok(())
}

fn collect_plugins(body: &[Token], parsed: &mut Parsed) {
    for statement in statements(body) {
        let sig = significant(statement);
        let Some(first) = sig.first() else {
            continue;
        };
        match &first.tok {
            Tok::Ident(name) if name == "id" => {
                if let Some((raw, _)) = sig.iter().skip(1).find_map(|token| token.string()) {
                    push_plugin(parsed, raw);
                    if raw == "org.springframework.boot" {
                        parsed.spring_boot = plugin_version(&sig);
                    }
                }
            }
            Tok::Ident(name) if name == "kotlin" => {
                if let Some((raw, _)) = sig.iter().skip(1).find_map(|token| token.string()) {
                    push_plugin(parsed, &format!("kotlin-{raw}"));
                }
            }
            Tok::Ident(name) if name != "plugins" && name != "version" && name != "apply" => {
                push_plugin(parsed, name);
            }
            _ => {}
        }
    }
}

/// The version that follows `id("plugin") version "x"` (or the Groovy form).
fn plugin_version(sig: &[&Token]) -> Option<String> {
    let index = sig
        .iter()
        .position(|token| token.ident() == Some("version"))?;
    sig.iter().skip(index + 1).find_map(|token| {
        token
            .string()
            .filter(|(_, interpolated)| !interpolated)
            .map(|(text, _)| text.to_string())
    })
}

fn push_plugin(parsed: &mut Parsed, name: &str) {
    if !parsed.plugins.iter().any(|existing| existing == name) {
        parsed.plugins.push(name.to_string());
    }
}

fn parse_top_level_assignments(
    source: &str,
    tokens: &[Token],
    parsed: &mut Parsed,
) -> Result<(), JxError> {
    for statement in statements(tokens) {
        let sig = significant(statement);
        let Some(first) = sig.first().and_then(|token| token.ident()) else {
            continue;
        };
        if first == "ext" {
            collect_ext(source, statement, &sig, &mut parsed.vars)?;
            continue;
        }
        if matches!(first, "def" | "val" | "var") {
            if let Some(name) = sig.get(1).and_then(|token| token.ident())
                && let Some(value) = assigned_string(source, &sig, &parsed.vars)?
            {
                parsed.vars.insert(name.to_string(), value);
            }
            continue;
        }
        match first {
            "version" => {
                if parsed.version.is_none()
                    && let Some(value) = assigned_string(source, &sig, &parsed.vars)?
                {
                    parsed.version = Some(value);
                    parsed.version_source = Some(format!("{source} version"));
                }
            }
            "group" => {
                if parsed.group.is_none() {
                    parsed.group = assigned_string(source, &sig, &parsed.vars)?;
                }
            }
            "mainClassName" => {
                if parsed.main_class.is_none() {
                    parsed.main_class = assigned_string(source, &sig, &parsed.vars)?;
                }
            }
            "java" => {
                if parsed.jdk.is_none()
                    && let Some(found) =
                        detect_jdk(&sig.iter().map(|t| (*t).clone()).collect::<Vec<_>>())
                {
                    parsed.jdk = Some(found);
                }
            }
            "sourceCompatibility" | "targetCompatibility" | "release" => {
                if parsed.jdk.is_none()
                    && let Some(feature) = assigned_java_version(&sig)
                {
                    parsed.jdk = Some((feature, format!("{first} = {feature}")));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn assigned_string(
    source: &str,
    sig: &[&Token],
    vars: &HashMap<String, String>,
) -> Result<Option<String>, JxError> {
    let start = sig
        .iter()
        .position(|token| matches!(token.tok, Tok::Punct('=')))
        .map(|index| index + 1)
        .unwrap_or(1);
    for token in sig.iter().skip(start) {
        if let Some((text, interpolated)) = token.string() {
            let value = if interpolated {
                substitute(text, vars).ok_or_else(|| dynamic_string(source, token.line, text))?
            } else {
                text.to_string()
            };
            return Ok(Some(value).filter(|text| !text.is_empty()));
        }
    }
    Ok(None)
}

/// `ext.name = "value"`, `ext { name = "value" }`, `def/val/var name = "value"`.
fn collect_ext(
    source: &str,
    statement: &[Token],
    sig: &[&Token],
    vars: &mut HashMap<String, String>,
) -> Result<(), JxError> {
    match sig.get(1).map(|token| &token.tok) {
        Some(Tok::Punct('.')) => {
            if let Some(name) = sig.get(2).and_then(|token| token.ident())
                && let Some(value) = assigned_string(source, sig, vars)?
            {
                vars.insert(name.to_string(), value);
            }
        }
        Some(Tok::Punct('{')) => {
            if let Some(open) = statement
                .iter()
                .position(|token| matches!(token.tok, Tok::Punct('{')))
                && let Some((start, end)) = block_at(statement, open)
            {
                for inner in statements(&statement[start..end]) {
                    let inner_sig = significant(inner);
                    if let Some(name) = inner_sig.first().and_then(|token| token.ident())
                        && let Some(value) = assigned_string(source, &inner_sig, vars)?
                    {
                        vars.insert(name.to_string(), value);
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Replace `${name}` / `$name` with literal variables collected from the
/// build. Returns `None` for anything the static reader cannot evaluate, so
/// the caller can fail instead of writing a partial coordinate.
fn substitute(raw: &str, vars: &HashMap<String, String>) -> Option<String> {
    substitute_depth(raw, vars, 0)
}

fn substitute_depth(raw: &str, vars: &HashMap<String, String>, depth: usize) -> Option<String> {
    if depth > 8 || !raw.contains('$') {
        return Some(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&next) = chars.peek() {
            if next.is_ascii_alphanumeric() || next == '_' {
                name.push(next);
                chars.next();
            } else {
                break;
            }
        }
        if braced && chars.next() != Some('}') {
            return None;
        }
        if name.is_empty() {
            return None;
        }
        let value = vars.get(&name)?;
        out.push_str(&substitute_depth(value, vars, depth + 1)?);
    }
    Some(out)
}

fn detected_version(raw: &str) -> Option<u32> {
    let raw = raw.trim().trim_matches(|c| c == '\'' || c == '"');
    let raw = raw.strip_prefix("VERSION_").unwrap_or(raw);
    parse_java_version(&raw.replace('_', "."))
}

fn assigned_java_version(sig: &[&Token]) -> Option<u32> {
    let start = sig
        .iter()
        .position(|token| matches!(token.tok, Tok::Punct('=')))
        .map(|index| index + 1)
        .unwrap_or(1);
    for token in sig.iter().skip(start) {
        match &token.tok {
            Tok::Num(number) => return detected_version(number),
            Tok::Str { text, .. } => return detected_version(text),
            Tok::Ident(name) => {
                if let Some(feature) = detected_version(name) {
                    return Some(feature);
                }
            }
            _ => {}
        }
    }
    None
}

/// Java toolchain / compatibility detection with a precedence order: a
/// toolchain language version wins, then release, then target, then source.
fn detect_jdk(tokens: &[Token]) -> Option<(u32, String)> {
    for key in [
        "languageVersion",
        "release",
        "targetCompatibility",
        "sourceCompatibility",
    ] {
        for (i, token) in tokens.iter().enumerate() {
            if token.ident() != Some(key) {
                continue;
            }
            let window = &tokens[i..tokens.len().min(i + 16)];
            if key == "languageVersion" {
                for (j, inner) in window.iter().enumerate() {
                    if inner.ident() == Some("of")
                        && let Some(Tok::Punct('(')) = window.get(j + 1).map(|t| &t.tok)
                        && let Some(Tok::Num(feature)) = window.get(j + 2).map(|t| &t.tok)
                        && let Ok(feature) = feature.parse::<u32>()
                    {
                        return Some((feature, format!("toolchain languageVersion = {feature}")));
                    }
                }
            } else if let Some(feature) = assigned_java_version_from(window) {
                return Some((feature, format!("{key} = {feature}")));
            }
        }
    }
    None
}

fn assigned_java_version_from(tokens: &[Token]) -> Option<u32> {
    let mut seen_equals = false;
    for token in tokens {
        match &token.tok {
            Tok::Punct('=') => seen_equals = true,
            Tok::Num(number) if seen_equals => return detected_version(number),
            Tok::Str { text, .. } if seen_equals => return detected_version(text),
            Tok::Ident(name) if seen_equals => {
                if let Some(feature) = detected_version(name) {
                    return Some(feature);
                }
            }
            Tok::Newline => {}
            _ if seen_equals => {}
            _ => {}
        }
        if seen_equals && matches!(token.tok, Tok::Punct(';')) {
            break;
        }
    }
    None
}

fn detect_main_class(tokens: &[Token]) -> Option<String> {
    let sig: Vec<&Token> = tokens
        .iter()
        .filter(|token| !matches!(token.tok, Tok::Newline))
        .collect();
    for (i, token) in sig.iter().enumerate() {
        match token.ident() {
            Some("mainClass") => {
                if let Some(value) = string_after_assignment(&sig, i) {
                    return Some(value);
                }
                // mainClass.set("…")
                if sig
                    .get(i + 1)
                    .is_some_and(|t| matches!(t.tok, Tok::Punct('.')))
                    && sig.get(i + 2).and_then(|t| t.ident()) == Some("set")
                {
                    return sig
                        .iter()
                        .skip(i + 3)
                        .find_map(|token| token.string().map(|(text, _)| text.to_string()));
                }
            }
            Some("mainClassName") => {
                if let Some(value) = string_after_assignment(&sig, i) {
                    return Some(value);
                }
            }
            _ => {}
        }
    }
    None
}

fn string_after_assignment(sig: &[&Token], index: usize) -> Option<String> {
    let mut seen_equals = false;
    for token in sig.iter().skip(index + 1) {
        match &token.tok {
            Tok::Punct('=') => seen_equals = true,
            Tok::Str { text, interpolated } if seen_equals => {
                if *interpolated {
                    return None;
                }
                return Some(text.to_string());
            }
            Tok::Newline => {}
            _ if !seen_equals => {}
            _ => return None,
        }
    }
    None
}

fn settings_root_name(tokens: &[Token]) -> Option<String> {
    let sig: Vec<&Token> = tokens
        .iter()
        .filter(|token| !matches!(token.tok, Tok::Newline))
        .collect();
    for (i, token) in sig.iter().enumerate() {
        if token.ident() == Some("rootProject")
            && sig
                .get(i + 1)
                .is_some_and(|t| matches!(t.tok, Tok::Punct('.')))
            && sig.get(i + 2).and_then(|t| t.ident()) == Some("name")
            && let Some(value) = string_after_assignment(&sig, i + 2)
        {
            return Some(value);
        }
    }
    None
}

fn settings_includes(tokens: &[Token]) -> bool {
    tokens.iter().enumerate().any(|(i, token)| {
        token.ident() == Some("include")
            && tokens
                .iter()
                .skip(i + 1)
                .take(3)
                .any(|next| next.string().is_some() || matches!(next.tok, Tok::Punct('(')))
    })
}

fn property(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

struct ManifestHeader<'a> {
    name: &'a str,
    version: &'a str,
    jdk: u32,
}

/// Everything `render_gradle` needs, kept in one struct so the renderer
/// does not grow a parameter per manifest section.
struct GradleRender<'a> {
    header: ManifestHeader<'a>,
    main: &'a [Dep],
    dev: &'a [Dep],
    processors: &'a [Dep],
    overrides: &'a [Dep],
    main_class: Option<&'a str>,
    framework: Option<&'a str>,
    source: &'a str,
}

fn render_gradle(request: GradleRender<'_>) -> String {
    let ManifestHeader { name, version, jdk } = request.header;
    let GradleRender {
        main,
        dev,
        processors,
        overrides,
        main_class,
        framework,
        source,
        ..
    } = request;
    let source = source.replace(['\r', '\n'], " ");
    let mut out = String::new();
    out.push_str("# Imported from ");
    out.push_str(&source);
    out.push_str(" by `jx import`.\n");
    out.push_str("# Direct dependencies are exact-pinned. Review the import notes,\n");
    out.push_str("# then run `jx lock` and `jx build` to verify the conversion.\n\n");
    out.push_str("[project]\n");
    out.push_str(&format!("name = {}\n", toml_string(name)));
    out.push_str(&format!("version = {}\n", toml_string(version)));
    out.push_str(&format!("jdk = {}\n", toml_string(&jdk.to_string())));
    if let Some(framework) = framework {
        out.push_str("\n[package]\n");
        out.push_str(&format!("framework = {}\n", toml_string(framework)));
    }

    let mut used = std::collections::HashSet::new();
    if !main.is_empty() {
        out.push_str("\n[dependencies]\n");
        for dep in main {
            push_gradle_dependency(&mut out, dep, &mut used);
        }
    }
    if !dev.is_empty() {
        out.push_str("\n[dev-dependencies]\n");
        for dep in dev {
            push_gradle_dependency(&mut out, dep, &mut used);
        }
    }
    if !processors.is_empty() {
        out.push_str("\n[processors]\n");
        for dep in processors {
            push_gradle_dependency(&mut out, dep, &mut used);
        }
    }
    if !overrides.is_empty() {
        out.push_str("\n[overrides]\n");
        for dep in overrides {
            out.push_str(&format!(
                "{} = {}\n",
                toml_string(&format!("{}:{}", dep.group_id, dep.artifact_id)),
                toml_string(dep.version.as_deref().unwrap_or_default())
            ));
        }
    }
    if let Some(main_class) = main_class {
        out.push_str("\n[run]\n");
        out.push_str(&format!("main = {}\n", toml_string(main_class)));
    }
    out
}

fn push_gradle_dependency(
    out: &mut String,
    dep: &Dep,
    used: &mut std::collections::HashSet<String>,
) {
    if dep.platform {
        // The platform entry reuses the same alias rules as a normal dependency.
        let mut alias = dep.artifact_id.clone();
        let mut suffix = 2;
        while used.contains(&alias) {
            alias = format!("{}-{suffix}", dep.artifact_id);
            suffix += 1;
        }
        used.insert(alias.clone());
        out.push_str(&format!(
            "{} = {{ platform = {} }}\n",
            toml_string(&alias),
            toml_string(&coordinate(dep))
        ));
    } else {
        let placeholder = crate::ImportedDependency {
            group_id: dep.group_id.clone(),
            artifact_id: dep.artifact_id.clone(),
            version: dep.version.clone().unwrap_or_default(),
            classifier: dep.classifier.clone(),
        };
        if dep.version.is_some() {
            push_dependency(out, &placeholder, used);
        } else {
            let mut alias = dep.artifact_id.clone();
            let mut suffix = 2;
            while used.contains(&alias) {
                alias = format!("{}-{suffix}", dep.artifact_id);
                suffix += 1;
            }
            used.insert(alias.clone());
            out.push_str(&format!(
                "{} = {}\n",
                toml_string(&alias),
                toml_string(&format!("{}:{}", dep.group_id, dep.artifact_id))
            ));
        }
    }
}

fn coordinate(dep: &Dep) -> String {
    let mut coordinate = format!("{}:{}", dep.group_id, dep.artifact_id);
    if let Some(version) = &dep.version {
        coordinate.push_str(":=");
        coordinate.push_str(version);
    }
    if let Some(classifier) = &dep.classifier {
        coordinate.push(':');
        coordinate.push_str(classifier);
    }
    coordinate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> GradleSource {
        GradleSource {
            script: "build.gradle.kts".into(),
            text: text.into(),
            settings: None,
            properties: None,
            dir_name: "app".into(),
            default_version: "0.1.0".into(),
            default_feature: 25,
        }
    }

    #[test]
    fn kotlin_build_maps_dependencies_platforms_and_main_class() {
        let text = r#"
plugins {
    application
    id("org.springframework.boot") version "3.4.0"
}

group = "com.example"
version = "1.2.0"

java {
    toolchain {
        languageVersion = JavaLanguageVersion.of(21)
    }
}

repositories {
    mavenCentral()
}

dependencies {
    implementation(platform("org.springframework.boot:spring-boot-dependencies:3.4.0"))
    implementation("org.springframework.boot:spring-boot-starter-web")
    implementation("com.google.guava:guava:33.4.8-jre")
    runtimeOnly("org.postgresql:postgresql:42.7.4")
    compileOnly("jakarta.servlet:jakarta.servlet-api:6.0.0")
    annotationProcessor("org.example:processor:1.0")
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")

    constraints {
        implementation("org.slf4j:slf4j-api:2.0.16")
    }
}

application {
    mainClass = "com.example.App"
}

tasks.test {
    useJUnitPlatform()
}
"#;
        let imported = manifest_from_gradle(&source(text)).expect("import");
        assert!(
            imported.text.contains("name = \"app\""),
            "{}",
            imported.text
        );
        assert!(imported.text.contains("version = \"1.2.0\""));
        assert!(imported.text.contains("jdk = \"21\""));
        assert!(imported.text.contains("jdk = \"21\""));
        assert!(imported.text.contains(
            "\"spring-boot-dependencies\" = { platform = \"org.springframework.boot:spring-boot-dependencies:=3.4.0\" }"
        ));
        assert!(imported.text.contains(
            "\"spring-boot-starter-web\" = \"org.springframework.boot:spring-boot-starter-web\""
        ));
        assert!(
            imported
                .text
                .contains("\"guava\" = \"com.google.guava:guava:=33.4.8-jre\"")
        );
        assert!(
            imported
                .text
                .contains("\"postgresql\" = \"org.postgresql:postgresql:=42.7.4\"")
        );
        assert!(
            imported
                .text
                .contains("[overrides]\n\"org.slf4j:slf4j-api\" = \"2.0.16\"")
        );
        assert!(imported.text.contains("[run]\nmain = \"com.example.App\""));
        assert!(
            imported
                .text
                .contains("[dev-dependencies]\n\"junit-jupiter\"")
        );
        assert!(imported.dependencies == 4);
        assert!(imported.dev_dependencies == 1);
        assert_eq!(imported.processors, 1);
        assert!(
            imported
                .text
                .contains("[processors]\n\"processor\" = \"org.example:processor:=1.0\"\n"),
            "{}",
            imported.text
        );
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("imported into [processors]"))
        );
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("Gradle plugin"))
        );
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("junit-platform-console-standalone"))
        );
    }

    #[test]
    fn annotation_processors_with_versions_are_imported() {
        let text = r#"
dependencies {
    implementation("com.google.guava:guava:33.4.8-jre")
    annotationProcessor("org.example:processor:1.0")
    testAnnotationProcessor("org.example:processor:1.0")
    annotationProcessor("org.example:versionless")
    kapt("org.example:kotlin-processor:2.0")
}
"#;
        let imported = manifest_from_gradle(&source(text)).expect("import");
        assert!(
            imported
                .text
                .contains("[processors]\n\"processor\" = \"org.example:processor:=1.0\"\n"),
            "{}",
            imported.text
        );
        // One coordinate declared for both compilations is written once.
        assert_eq!(imported.processors, 1);
        assert_eq!(imported.text.matches("org.example:processor").count(), 1);
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("imported into [processors]")),
            "{:?}",
            imported.notes
        );
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("were not imported")),
            "{:?}",
            imported.notes
        );
        assert!(!imported.text.contains("versionless"));
        assert!(!imported.text.contains("kotlin-processor"));
    }

    #[test]
    fn groovy_build_supports_strings_and_map_notation() {
        let text = r#"
plugins {
    id 'java'
}

version = '2.0.0'

sourceCompatibility = JavaVersion.VERSION_17

dependencies {
    implementation 'com.google.guava:guava:33.4.8-jre'
    implementation group: 'org.slf4j', name: 'slf4j-api', version: '2.0.16'
    testImplementation 'org.junit.jupiter:junit-jupiter:5.11.4'
    testRuntimeOnly 'org.junit.platform:junit-platform-launcher:1.11.4'
}
"#;
        let imported = manifest_from_gradle(&source(text)).expect("import");
        assert!(imported.text.contains("jdk = \"17\""));
        assert!(imported.text.contains("version = \"2.0.0\""));
        assert!(
            imported
                .text
                .contains("\"slf4j-api\" = \"org.slf4j:slf4j-api:=2.0.16\"")
        );
        assert!(imported.text.contains("[dev-dependencies]"));
        assert_eq!(imported.dependencies, 2);
        assert_eq!(imported.dev_dependencies, 2);
    }

    #[test]
    fn settings_name_and_properties_version_are_used() {
        let mut build = source(
            r#"
dependencies {
    implementation("com.google.guava:guava:33.4.8-jre")
}
"#,
        );
        build.settings = Some(("settings.gradle.kts".into(), create_settings()));
        build.properties = Some(("gradle.properties".into(), "version=3.1.4\n".into()));
        let imported = manifest_from_gradle(&build).expect("import");
        assert!(imported.text.contains("name = \"service\""));
        assert!(imported.text.contains("version = \"3.1.4\""));
        assert!(imported.text.contains("jdk = \"25\""));
    }

    fn create_settings() -> String {
        "rootProject.name = \"service\"\ninclude(\"core\", \"web\")\n".to_string()
    }

    #[test]
    fn multi_module_settings_are_noted_not_guessed() {
        let mut build = source("dependencies {\n    implementation(\"g:a:1.0\")\n}\n");
        build.settings = Some(("settings.gradle.kts".into(), create_settings()));
        let imported = manifest_from_gradle(&build).expect("import");
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("only this module was imported"))
        );
    }

    #[test]
    fn versionless_dependency_without_platform_is_refused() {
        let error = manifest_from_gradle(&source(
            "dependencies {\n    implementation(\"com.example:app\")\n}\n",
        ))
        .expect_err("no version");
        assert!(error.message.contains("has no version"), "{error}");
        assert!(error.hint.is_some());
    }

    #[test]
    fn literal_ext_variables_are_substituted() {
        let text = r#"
ext.webjars = '1.1.3'
ext {
    bootstrap = "5.3.8"
}

dependencies {
    implementation "org.webjars:webjars-locator-lite:${webjars}"
    implementation('org.webjars.npm:bootstrap:' + bootstrap)
}
"#;
        // The second line concatenates, which is still script evaluation.
        let error = manifest_from_gradle(&source(text)).expect_err("concatenation");
        assert!(!error.message.is_empty());

        let ok = r#"
ext.webjars = '1.1.3'
ext {
    bootstrap = "5.3.8"
}

dependencies {
    implementation "org.webjars:webjars-locator-lite:${webjars}"
    implementation "org.webjars.npm:bootstrap:${bootstrap}"
}
"#;
        let imported = manifest_from_gradle(&source(ok)).expect("import");
        assert!(
            imported
                .text
                .contains("\"webjars-locator-lite\" = \"org.webjars:webjars-locator-lite:=1.1.3\"")
        );
        assert!(
            imported
                .text
                .contains("\"bootstrap\" = \"org.webjars.npm:bootstrap:=5.3.8\"")
        );
    }

    #[test]
    fn spring_boot_plugin_supplies_versions_through_its_bom() {
        let text = r#"
plugins {
    id 'org.springframework.boot' version '3.4.1'
    id 'io.spring.dependency-management' version '1.1.7'
    id 'java'
}

version = '3.4.1-SNAPSHOT'

java {
    toolchain {
        languageVersion = JavaLanguageVersion.of(21)
    }
}

dependencies {
    implementation 'org.springframework.boot:spring-boot-starter-web'
    testImplementation 'org.springframework.boot:spring-boot-starter-test'
}
"#;
        let imported = manifest_from_gradle(&source(text)).expect("import");
        assert!(imported.text.contains(
            "\"spring-boot-dependencies\" = { platform = \"org.springframework.boot:spring-boot-dependencies:=3.4.1\" }"
        ));
        assert!(imported.text.contains(
            "\"spring-boot-starter-web\" = \"org.springframework.boot:spring-boot-starter-web\""
        ));
        assert!(imported.text.contains("[dev-dependencies]"));
        assert!(
            imported
                .text
                .contains("[package]\nframework = \"spring-boot\""),
            "the Boot plugin marks the module a Boot app: {}",
            imported.text
        );
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("Spring Boot plugin"))
        );
    }

    #[test]
    fn quarkus_dependency_selects_the_framework() {
        let text = r#"
plugins {
    id 'java'
}

dependencies {
    implementation 'io.quarkus:quarkus-core:3.20.0'
    testImplementation 'io.vertx:vertx-junit5:5.0.0'
}
"#;
        let imported = manifest_from_gradle(&source(text)).expect("import");
        assert!(
            imported.text.contains("[package]\nframework = \"quarkus\""),
            "{}",
            imported.text
        );
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("io.quarkus"))
        );
    }

    #[test]
    fn spring_boot_without_a_version_cannot_guess_the_bom() {
        let text = r#"
plugins {
    id 'org.springframework.boot'
}

dependencies {
    implementation 'org.springframework.boot:spring-boot-starter-web'
}
"#;
        let error = manifest_from_gradle(&source(text)).expect_err("no BOM version");
        assert!(error.message.contains("has no version"), "{error}");
    }

    #[test]
    fn test_platform_import_supplies_both_tables() {
        let text = r#"
dependencies {
    testImplementation(platform("org.junit:junit-bom:5.11.4"))
    testImplementation("org.junit.jupiter:junit-jupiter")
}
"#;
        let imported = manifest_from_gradle(&source(text)).expect("import");
        // Platform entries live in [dependencies] because jx applies them to
        // both tables.
        assert!(imported.text.contains(
            "[dependencies]\n\"junit-bom\" = { platform = \"org.junit:junit-bom:=5.11.4\" }"
        ));
        assert!(imported.text.contains(
            "[dev-dependencies]\n\"junit-jupiter\" = \"org.junit.jupiter:junit-jupiter\""
        ));
    }

    #[test]
    fn interpolation_and_project_dependencies_are_refused() {
        for text in [
            "dependencies {\n    implementation(\"com.example:app:$version\")\n}\n",
            "dependencies {\n    implementation(project(\":core\"))\n}\n",
            "dependencies {\n    implementation(libs.guava)\n}\n",
        ] {
            let error = manifest_from_gradle(&source(text)).expect_err("dynamic");
            assert!(!error.message.is_empty(), "{text}");
        }
    }

    #[test]
    fn dynamic_control_flow_inside_dependencies_is_refused() {
        let error = manifest_from_gradle(&source(
            "dependencies {\n    if (true) {\n        implementation(\"g:a:1.0\")\n    }\n}\n",
        ))
        .expect_err("control flow");
        assert!(error.message.contains("cannot be imported statically"));
    }

    #[test]
    fn duplicate_dependency_is_refused() {
        let error = manifest_from_gradle(&source(
            "dependencies {\n    implementation(\"g:a:1.0\")\n    implementation(\"g:a:2.0\")\n}\n",
        ))
        .expect_err("duplicate");
        assert!(error.message.contains("more than once"));
    }

    #[test]
    fn dynamic_versions_are_refused() {
        for version in ["1.+", "[1.0,2.0)", "latest.release", "1.0-SNAPSHOT"] {
            let text = format!("dependencies {{\n    implementation(\"g:a:{version}\")\n}}\n");
            assert!(manifest_from_gradle(&source(&text)).is_err(), "{version}");
        }
    }

    #[test]
    fn test_dependency_that_is_also_main_stays_main() {
        let imported = manifest_from_gradle(&source(
            "dependencies {\n    implementation(\"g:a:1.0\")\n    testImplementation(\"g:a:1.0\")\n}\n",
        ))
        .expect("import");
        assert!(imported.text.contains("[dependencies]"));
        assert!(!imported.text.contains("[dev-dependencies]"));
        assert_eq!(imported.dependencies, 1);
        assert_eq!(imported.dev_dependencies, 0);
    }

    #[test]
    fn unknown_configurations_are_skipped_with_a_note() {
        let imported = manifest_from_gradle(&source(
            "dependencies {\n    customImplementation(\"g:custom:1.0\")\n    implementation(\"g:a:1.0\")\n}\n",
        ))
        .expect("import");
        assert_eq!(imported.dependencies, 1);
        assert!(
            imported
                .notes
                .iter()
                .any(|note| note.contains("customImplementation"))
        );
    }
}
