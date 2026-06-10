use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use regex::Regex;
use scraper::{Html, Selector};
use sevenz_rust2::decompress_file as decompress_7z_file;
use tokimo_package_client_api::assrt::{ASSRT_BASE_URL, ASSRT_USER_AGENT, AssrtClient, AssrtConfig};
use tokio::{fs, process::Command};
use unrar::Archive as UnrarArchive;
use uuid::Uuid;
use zip::ZipArchive;

use crate::models::{SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult};

#[derive(Debug, Clone)]
pub struct DownloadedSubtitle {
    pub name: String,
    pub format: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ExtractedSubtitleFile {
    name: String,
    format: String,
    content: Vec<u8>,
}

#[derive(Debug, Clone)]
struct DetailSubtitleFile {
    name: String,
    format: String,
    url: String,
}

fn normalize_language(language_text: &str) -> String {
    let normalized = language_text.split_whitespace().collect::<String>();
    if normalized.contains('简') {
        return "zh-CN".into();
    }
    if normalized.contains('繁') {
        return "zh-TW".into();
    }
    if normalized.contains("双语") || normalized.contains("雙語") {
        return "zh".into();
    }
    if normalized.contains('英') {
        return "en".into();
    }
    if normalized.contains('日') {
        return "ja".into();
    }
    if normalized.contains('韩') || normalized.contains('韓') {
        return "ko".into();
    }
    "zh".into()
}

fn normalize_format(format_text: &str) -> Option<String> {
    let lowered = format_text.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return None;
    }
    if lowered.contains("subrip") || lowered == "srt" {
        return Some("srt".into());
    }
    if lowered.contains("advanced substation alpha") || lowered == "ass" {
        return Some("ass".into());
    }
    if lowered.contains("substation alpha") || lowered == "ssa" {
        return Some("ssa".into());
    }
    if lowered.contains("webvtt") || lowered == "vtt" {
        return Some("vtt".into());
    }

    match lowered.rsplit('.').next() {
        Some("srt") => Some("srt".into()),
        Some("ass") => Some("ass".into()),
        Some("ssa") => Some("ssa".into()),
        Some("vtt") => Some("vtt".into()),
        _ => None,
    }
}

fn matches_preferred_language(language: &str, preferred_languages: Option<&[String]>) -> bool {
    let Some(preferred_languages) = preferred_languages else {
        return true;
    };
    if preferred_languages.is_empty() {
        return true;
    }
    if preferred_languages.iter().any(|preferred| preferred == language) {
        return true;
    }
    if language == "zh" {
        return preferred_languages.iter().any(|preferred| preferred.starts_with("zh"));
    }
    if language.starts_with("zh-") {
        return preferred_languages.iter().any(|preferred| preferred == "zh");
    }
    false
}

fn score_subtitle_name(name: &str, format: &str, preferred_language: &str) -> i32 {
    let lower = name.to_ascii_lowercase();
    let mut score = 0;

    if preferred_language == "zh-CN" {
        if lower.contains("chs") || name.contains('简') {
            score += 40;
        }
        if lower.contains("eng") || name.contains("双语") || name.contains("雙語") {
            score += 10;
        }
    } else if preferred_language == "zh-TW" {
        if lower.contains("cht") || name.contains('繁') {
            score += 40;
        }
        if lower.contains("eng") || name.contains("双语") || name.contains("雙語") {
            score += 10;
        }
    } else if preferred_language == "en" {
        if lower.contains("eng") || name.contains("英文") {
            score += 40;
        }
    } else if preferred_language == "ja" {
        if lower.contains("jpn") || name.contains('日') {
            score += 40;
        }
    } else if preferred_language == "ko" {
        if lower.contains("kor") || name.contains('韩') || name.contains('韓') {
            score += 40;
        }
    } else if lower.contains("chs") || lower.contains("cht") || name.contains("双语") || name.contains("雙語") {
        score += 20;
    }

    score
        + match format {
            "srt" => 8,
            "ass" => 6,
            "ssa" => 4,
            "vtt" => 2,
            _ => 0,
        }
}

fn absolute_assrt_url(path: &str) -> String {
    tokimo_package_client_api::assrt::absolute_url(path)
}

fn text_content(element: scraper::ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join("")
        .replace('\u{a0}', " ")
        .trim()
        .to_string()
}

fn filename_from_path(path: &str, fallback: &str) -> String {
    path.rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn temporary_archive_name(archive_name: &str) -> String {
    let extension = archive_name
        .rsplit('.')
        .next()
        .filter(|value| !value.is_empty() && *value != archive_name)
        .unwrap_or("bin");
    format!("archive.{extension}")
}

fn archive_extension(archive_name: &str) -> Option<String> {
    archive_name
        .rsplit('.')
        .next()
        .filter(|value| !value.is_empty() && *value != archive_name)
        .map(str::to_ascii_lowercase)
}

fn is_assrt_block_page(content: &[u8]) -> bool {
    content.starts_with(b"<?xml")
        || content.starts_with(b"<html>")
        || String::from_utf8_lossy(content).contains("/errpage/40x")
}

fn read_extracted_subtitles_from_dir(root: &Path) -> Result<Vec<ExtractedSubtitleFile>, String> {
    let mut directories = vec![root.to_path_buf()];
    let mut extracted_files = Vec::new();

    while let Some(directory) = directories.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| format!("读取 assrt 解压目录失败: {error}"))?;

        for entry in entries {
            let entry = entry.map_err(|error| format!("遍历 assrt 解压目录失败: {error}"))?;
            let path = entry.path();

            if path.is_dir() {
                directories.push(path);
                continue;
            }

            if !path.is_file() {
                continue;
            }

            let file_name = path
                .file_name()
                .map_or_else(|| "subtitle".into(), |value| value.to_string_lossy().to_string());
            let Some(format) = normalize_format(&file_name) else {
                continue;
            };
            let content = std::fs::read(&path).map_err(|error| format!("读取解压后的 assrt 字幕失败: {error}"))?;
            extracted_files.push(ExtractedSubtitleFile {
                name: file_name,
                format,
                content,
            });
        }
    }

    Ok(extracted_files)
}

fn extract_rar_subtitles(archive_path: &Path, output_dir: &Path) -> Result<Vec<ExtractedSubtitleFile>, String> {
    let archive = UnrarArchive::new(archive_path).as_first_part();
    let mut archive = archive
        .open_for_processing()
        .map_err(|error| format!("解压 assrt RAR 字幕失败: {error}"))?;

    while let Some(header) = archive
        .read_header()
        .map_err(|error| format!("读取 assrt RAR 条目失败: {error}"))?
    {
        archive = if header.entry().is_file() {
            let destination = output_dir.join(&header.entry().filename);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| format!("创建 assrt RAR 解压目录失败: {error}"))?;
            }
            header
                .extract_to(&destination)
                .map_err(|error| format!("提取 assrt RAR 字幕失败: {error}"))?
        } else {
            header
                .skip()
                .map_err(|error| format!("跳过 assrt RAR 条目失败: {error}"))?
        };
    }

    read_extracted_subtitles_from_dir(output_dir)
}

fn extract_zip_subtitles(archive_path: &Path) -> Result<Vec<ExtractedSubtitleFile>, String> {
    let file = File::open(archive_path).map_err(|error| format!("打开 assrt ZIP 压缩包失败: {error}"))?;
    let mut archive = ZipArchive::new(file).map_err(|error| format!("读取 assrt ZIP 压缩包失败: {error}"))?;
    let mut extracted_files = Vec::new();

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("读取 assrt ZIP 条目失败: {error}"))?;
        if !entry.is_file() {
            continue;
        }

        let file_name = entry
            .enclosed_name()
            .and_then(|path| path.file_name().map(|value| value.to_string_lossy().to_string()))
            .unwrap_or_else(|| entry.name().to_string());
        let Some(format) = normalize_format(&file_name) else {
            continue;
        };

        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|error| format!("读取 assrt ZIP 字幕内容失败: {error}"))?;
        extracted_files.push(ExtractedSubtitleFile {
            name: file_name,
            format,
            content,
        });
    }

    Ok(extracted_files)
}

fn extract_7z_subtitles(archive_path: &Path, output_dir: &Path) -> Result<Vec<ExtractedSubtitleFile>, String> {
    decompress_7z_file(archive_path, output_dir).map_err(|error| format!("解压 assrt 7z 字幕失败: {error}"))?;

    read_extracted_subtitles_from_dir(output_dir)
}

fn parse_detail_download_files(detail_html: &str, _subtitle_id: &str) -> Result<Vec<DetailSubtitleFile>, String> {
    let onthefly_regex = Regex::new(r#"onthefly\("(\d+)","(\d+)","([^"]+)"\)"#)
        .map_err(|error| format!("detail file regex failed: {error}"))?;
    let mut files = Vec::new();

    for captures in onthefly_regex.captures_iter(detail_html) {
        let Some(subtitle_id_value) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        let Some(index) = captures.get(2).map(|value| value.as_str()) else {
            continue;
        };
        let Some(name) = captures.get(3).map(|value| value.as_str().trim().to_string()) else {
            continue;
        };
        let Some(format) = normalize_format(&name) else {
            continue;
        };
        let url = {
            let mut download_url =
                url::Url::parse(ASSRT_BASE_URL).map_err(|error| format!("解析 assrt 下载基础地址失败: {error}"))?;
            download_url
                .path_segments_mut()
                .map_err(|()| "构造 assrt 下载地址失败".to_string())?
                .extend(["download", subtitle_id_value, "-", index, name.as_str()]);
            download_url.to_string()
        };
        files.push(DetailSubtitleFile {
            format,
            name: name.clone(),
            url,
        });
    }

    Ok(files)
}

pub async fn search_assrt_subtitles(request: &SubtitleSearchRequest) -> Result<Vec<SubtitleSearchResult>, String> {
    let query = request.query.clone().unwrap_or_default().trim().to_string();
    if query.is_empty() {
        return Err("请输入片名或文件名后再搜索 assrt 字幕".into());
    }

    let search_url = format!(
        "{ASSRT_BASE_URL}/sub/?searchword={}&sort=rank&no_redir=1",
        url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
    );
    let assrt_client = AssrtClient::new(AssrtConfig {
        http_client: reqwest::Client::builder()
            .user_agent(ASSRT_USER_AGENT)
            .build()
            .map_err(|e| format!("创建 assrt HTTP 客户端失败: {e}"))?,
    });
    let html = assrt_client.fetch_html(&search_url).await.map_err(|e| e.to_string())?;
    let document = Html::parse_document(&html);
    let row_selector =
        Selector::parse(".resultcard .subitem").map_err(|error| format!("解析 assrt 搜索选择器失败: {error}"))?;
    let title_selector =
        Selector::parse(".introtitle").map_err(|error| format!("解析 assrt 标题选择器失败: {error}"))?;
    let meta_span_selector =
        Selector::parse("#sublist_div span").map_err(|error| format!("解析 assrt 元数据选择器失败: {error}"))?;
    let version_selector =
        Selector::parse("#meta_top b").map_err(|error| format!("解析 assrt 版本选择器失败: {error}"))?;
    let rating_selector = Selector::parse(r#"img[alt*="用户评分"], img[title*="用户评分"]"#)
        .map_err(|error| format!("解析 assrt 评分选择器失败: {error}"))?;
    let detail_id_regex = Regex::new(r"/(\d+)\.xml$").map_err(|error| format!("detail id regex failed: {error}"))?;
    let download_regex =
        Regex::new(r"location\.href='([^']+)'").map_err(|error| format!("download path regex failed: {error}"))?;
    let number_regex = Regex::new(r"([\d.]+)").map_err(|error| format!("number regex failed: {error}"))?;
    let download_count_regex =
        Regex::new(r"下载次数：\s*(\d+)").map_err(|error| format!("download count regex failed: {error}"))?;

    let mut results = Vec::new();

    for row in document.select(&row_selector) {
        let Some(detail_anchor) = row.select(&title_selector).next() else {
            continue;
        };
        let detail_path = detail_anchor.value().attr("href").map(std::string::ToString::to_string);
        let Some(detail_path_value) = detail_path.as_deref() else {
            continue;
        };
        let Some(id_match) = detail_id_regex.captures(detail_path_value) else {
            continue;
        };
        let Some(id) = id_match.get(1).map(|value| value.as_str().to_string()) else {
            continue;
        };

        let span_texts = row.select(&meta_span_selector).map(text_content).collect::<Vec<_>>();
        let format_text = span_texts
            .iter()
            .find_map(|text| text.strip_prefix("格式："))
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let Some(format) = normalize_format(&format_text) else {
            continue;
        };
        let language_text = span_texts
            .iter()
            .find_map(|text| text.strip_prefix("语言："))
            .map_or("未知", str::trim)
            .to_string();
        let version_text = row
            .select(&version_selector)
            .next()
            .map(text_content)
            .unwrap_or_default();
        let download_count = span_texts
            .iter()
            .find_map(|text| download_count_regex.captures(text))
            .and_then(|captures| captures.get(1))
            .and_then(|value| value.as_str().parse::<u64>().ok());
        let rating = row
            .select(&rating_selector)
            .next()
            .and_then(|node| {
                node.value()
                    .attr("alt")
                    .or_else(|| node.value().attr("title"))
                    .map(str::to_string)
            })
            .and_then(|text| {
                number_regex
                    .captures(&text)
                    .and_then(|captures| captures.get(1).map(|value| value.as_str().to_string()))
            })
            .and_then(|value| value.parse::<f64>().ok());
        let download_path = download_regex
            .captures(&row.html())
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_string());
        let language = normalize_language(&language_text);
        if !matches_preferred_language(&language, request.languages.as_deref()) {
            continue;
        }

        results.push(SubtitleSearchResult {
            id,
            name: {
                let title = text_content(detail_anchor);
                if title.is_empty() {
                    if version_text.is_empty() {
                        query.clone()
                    } else {
                        version_text.clone()
                    }
                } else {
                    title
                }
            },
            language,
            language_name: language_text,
            format,
            provider: "assrt".into(),
            detail_path,
            download_path,
            download_count,
            rating,
            movie_name: detail_anchor
                .value()
                .attr("title")
                .map(|value| value.trim().to_string()),
            release_group: (!version_text.is_empty()).then_some(version_text),
        });
    }

    Ok(results)
}

async fn extract_archive(
    archive_bytes: &[u8],
    archive_name: &str,
    preferred_language: &str,
    staging_root: &Path,
) -> Result<DownloadedSubtitle, String> {
    let temp_root = staging_root.join("assrt-subtitles").join(Uuid::new_v4().to_string());
    let result = async {
        let output_dir = temp_root.join("out");
        fs::create_dir_all(&output_dir)
            .await
            .map_err(|error| format!("创建 assrt 临时目录失败: {error}"))?;

        let archive_path = temp_root.join(temporary_archive_name(archive_name));
        fs::write(&archive_path, archive_bytes)
            .await
            .map_err(|error| format!("写入 assrt 压缩包失败: {error}"))?;
        let archive_path_for_task = archive_path.clone();
        let output_dir_for_task = output_dir.clone();
        let archive_name_for_task = archive_name.to_string();
        let extracted_files = tokio::task::spawn_blocking(move || {
            if UnrarArchive::new(&archive_path_for_task).is_archive() {
                return extract_rar_subtitles(&archive_path_for_task, &output_dir_for_task);
            }

            match archive_extension(&archive_name_for_task).as_deref() {
                Some("zip") => extract_zip_subtitles(&archive_path_for_task),
                Some("7z") => extract_7z_subtitles(&archive_path_for_task, &output_dir_for_task),
                Some(extension) => Err(format!("暂不支持解压 {extension} 格式的 assrt 字幕包")),
                None => Err("无法识别 assrt 字幕压缩包格式".into()),
            }
        })
        .await
        .map_err(|error| format!("解压 assrt 字幕任务失败: {error}"))??;

        let selected = extracted_files
            .into_iter()
            .max_by_key(|file| score_subtitle_name(&file.name, &file.format, preferred_language))
            .ok_or_else(|| "assrt 压缩包内未找到可用字幕文件".to_string())?;

        Ok(DownloadedSubtitle {
            name: selected.name,
            format: selected.format,
            content: selected.content,
        })
    }
    .await;

    let _ = fs::remove_dir_all(&temp_root).await;
    result
}

fn assrt_cookie_jar_path(staging_root: &Path) -> PathBuf {
    staging_root.join("assrt-subtitles").join("cookies.txt")
}

async fn warm_assrt_session(cookie_jar: &Path) -> Result<(), String> {
    if let Some(parent) = cookie_jar.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("创建 assrt cookie 目录失败: {error}"))?;
    }

    let cookie_jar_str = cookie_jar.to_string_lossy().to_string();
    let accept_html = "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
    let warmup_output = Command::new("curl")
        .arg("-sSL")
        .arg("-A")
        .arg(ASSRT_USER_AGENT)
        .arg("-H")
        .arg(accept_html)
        .arg("-b")
        .arg(&cookie_jar_str)
        .arg("-c")
        .arg(&cookie_jar_str)
        .arg("-o")
        .arg("/dev/null")
        .arg(ASSRT_BASE_URL)
        .output()
        .await
        .map_err(|error| format!("调用 curl 预热 assrt 会话失败: {error}"))?;

    if !warmup_output.status.success() {
        return Err(format!(
            "curl 预热 assrt 会话失败: {}",
            String::from_utf8_lossy(&warmup_output.stderr).trim()
        ));
    }

    Ok(())
}

async fn fetch_assrt_detail_html_via_curl(detail_url: &str, cookie_jar: &Path) -> Result<String, String> {
    let cookie_jar_str = cookie_jar.to_string_lossy().to_string();
    let accept_html = "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
    let detail_output = Command::new("curl")
        .arg("-sSL")
        .arg("-A")
        .arg(ASSRT_USER_AGENT)
        .arg("-H")
        .arg(accept_html)
        .arg("-b")
        .arg(&cookie_jar_str)
        .arg("-c")
        .arg(&cookie_jar_str)
        .arg("-e")
        .arg(ASSRT_BASE_URL)
        .arg(detail_url)
        .output()
        .await
        .map_err(|error| format!("调用 curl 获取 assrt 详情页失败: {error}"))?;

    if !detail_output.status.success() {
        return Err(format!(
            "curl 获取 assrt 详情页失败: {}",
            String::from_utf8_lossy(&detail_output.stderr).trim()
        ));
    }

    String::from_utf8(detail_output.stdout).map_err(|error| format!("解码 assrt 详情页失败: {error}"))
}

async fn download_detail_subtitle_via_curl(
    detail_path: &str,
    subtitle_id: &str,
    preferred_language: &str,
    staging_root: &Path,
) -> Result<DownloadedSubtitle, String> {
    let cookie_jar = assrt_cookie_jar_path(staging_root);
    let cookie_jar_str = cookie_jar.to_string_lossy().to_string();
    let detail_url = absolute_assrt_url(detail_path);

    warm_assrt_session(&cookie_jar).await?;

    let mut detail_html = fetch_assrt_detail_html_via_curl(&detail_url, &cookie_jar).await?;
    if detail_html.contains("/errpage/40x") {
        warm_assrt_session(&cookie_jar).await?;
        detail_html = fetch_assrt_detail_html_via_curl(&detail_url, &cookie_jar).await?;
    }
    if detail_html.contains("/errpage/40x") {
        return Err("assrt 详情页请求失败: 493".into());
    }

    let detail_files = parse_detail_download_files(&detail_html, subtitle_id)?;
    let selected_file = detail_files
        .into_iter()
        .max_by_key(|file| score_subtitle_name(&file.name, &file.format, preferred_language))
        .ok_or_else(|| "assrt 详情页未提供可下载字幕文件".to_string())?;

    let mut file_output = Command::new("curl")
        .arg("-sSL")
        .arg("-A")
        .arg(ASSRT_USER_AGENT)
        .arg("-H")
        .arg("Accept: */*")
        .arg("-b")
        .arg(&cookie_jar_str)
        .arg("-c")
        .arg(&cookie_jar_str)
        .arg("-e")
        .arg(&detail_url)
        .arg(&selected_file.url)
        .output()
        .await
        .map_err(|error| format!("调用 curl 下载 assrt 字幕失败: {error}"))?;

    if !file_output.status.success() {
        return Err(format!(
            "curl 下载 assrt 字幕失败: {}",
            String::from_utf8_lossy(&file_output.stderr).trim()
        ));
    }

    if is_assrt_block_page(&file_output.stdout) {
        warm_assrt_session(&cookie_jar).await?;
        file_output = Command::new("curl")
            .arg("-sSL")
            .arg("-A")
            .arg(ASSRT_USER_AGENT)
            .arg("-H")
            .arg("Accept: */*")
            .arg("-b")
            .arg(&cookie_jar_str)
            .arg("-c")
            .arg(&cookie_jar_str)
            .arg("-e")
            .arg(&detail_url)
            .arg(&selected_file.url)
            .output()
            .await
            .map_err(|error| format!("调用 curl 重试下载 assrt 字幕失败: {error}"))?;
    }

    if !file_output.status.success() {
        return Err(format!(
            "curl 重试下载 assrt 字幕失败: {}",
            String::from_utf8_lossy(&file_output.stderr).trim()
        ));
    }

    if is_assrt_block_page(&file_output.stdout) {
        return Err(format!("assrt 单文件下载失败: 493 ({})", selected_file.url));
    }

    Ok(DownloadedSubtitle {
        name: selected_file.name,
        format: selected_file.format,
        content: file_output.stdout,
    })
}

async fn download_archive_subtitle(
    request: &SubtitleDownloadRequest,
    staging_root: &Path,
) -> Result<DownloadedSubtitle, String> {
    let assrt_client = AssrtClient::new(AssrtConfig {
        http_client: reqwest::Client::builder()
            .user_agent(ASSRT_USER_AGENT)
            .build()
            .map_err(|e| format!("创建 assrt HTTP 客户端失败: {e}"))?,
    });
    let download_path = request
        .download_path
        .clone()
        .ok_or_else(|| "assrt 搜索结果缺少下载地址".to_string())?;
    let download_url = absolute_assrt_url(&download_path);
    let content = assrt_client
        .download_archive(&download_url)
        .await
        .map_err(|e| format!("assrt 请求失败: {e}"))?;

    let archive_file_name = filename_from_path(&download_path, &request.subtitle_id);
    let subtitle_name = request.name.clone().unwrap_or_else(|| archive_file_name.clone());

    if let Some(format) = normalize_format(&archive_file_name) {
        return Ok(DownloadedSubtitle {
            name: subtitle_name,
            format,
            content: content.to_vec(),
        });
    }

    extract_archive(&content, &archive_file_name, &request.language, staging_root).await
}

pub async fn download_assrt_subtitle(
    request: &SubtitleDownloadRequest,
    staging_root: &Path,
) -> Result<DownloadedSubtitle, String> {
    if let Some(detail_path) = request.detail_path.as_deref() {
        match download_detail_subtitle_via_curl(detail_path, &request.subtitle_id, &request.language, staging_root)
            .await
        {
            Ok(subtitle) => return Ok(subtitle),
            Err(detail_error) => {
                if request.download_path.is_none() {
                    return Err(detail_error);
                }
            }
        }
    }

    download_archive_subtitle(request, staging_root).await
}
