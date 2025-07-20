use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use dotenvy;
use anyhow::Result;
use mal_api::prelude::*;
use crunchyroll_rs::{Crunchyroll, Locale};
use crunchyroll_rs::common::StreamExt;
use reqwest::Response;
use std::{collections::HashSet, env, thread, time::Duration};

fn get_node_title(node: AnimeFields) -> String {
    match node.alternative_titles {
    Some(x) => {
        match x.en {
        Some(x) if x.len() > 0 => x,
        _ => node.title
        }
    },
    None => node.title
    }
}

async fn read_mal_entries() -> Result<Vec<AnimeListNode>> {
    let mal_username = env::var("MAL_USERNAME")
        .expect("'MAL_USERNAME' environment variable not found");

    let client_id = MalClientId::try_from_env()?;
    let api_client = AnimeApiClient::from(&client_id);
    
    let mut output: Vec<AnimeListNode> = vec![];
    let max_page_size = 1000;
    let mut offset = 0;
    let mut done = false;

    while !done {
        eprintln!("Reading");
        thread::sleep(Duration::from_secs(2));
        let query = GetUserAnimeList::builder(mal_username.as_str())
            .enable_nsfw()
            .offset(offset)
            .limit(max_page_size)
            .fields(&AnimeCommonFields(vec![
                AnimeField::list_status,
                AnimeField::title,
                AnimeField::alternative_titles,
                AnimeField::start_date
            ]))
            .sort(UserAnimeListSort::AnimeStartDate)
            .build()?;
        let res = api_client.get_user_anime_list(&query).await;
        match res {
            Err(e) => {
                eprintln!("Error while retrieving the list: {}", e);
                done = true;
            }
            Ok(r) => {
                done = r.data.len() != (max_page_size as usize);
                output.extend(r.data.into_iter()
                    .filter(|elt| {
                        let status = &elt.list_status;
                        if status.is_none() {
                            return false;
                        }
                        let status = status.as_ref().unwrap();
                        if status.num_episodes_watched == 0 {
                            return false;
                        }

                        true
                    })
                );
            }
        }

        offset += max_page_size as u32;
    }
    eprintln!("{} elements read", output.len());

    // We need to reverse the vector so the older seasons
    // appear first
    output.reverse();
    Ok(output)
}

fn same_title(p: &str, s: &str) -> bool {
    let n = p.len();
    if s.len() < n || n == 0 {
        return false;
    }
    /*
        We need the minimal edit distance here because there is
        discrepancies between MAL's naming & CR's naming.
        Ex.:
            - hitoribocchi no marumaru seikatsu vs. hitoribocchi no marumaruseikatsu
            - ...
        And the 0.125 value is just a guess. For a 20 letters title,
        the maximum distance is 2.
     */
    let score = (levenshtein::levenshtein(p, &s[..n]) as f32) / (n as f32);
    
    if score >= 0.01 {
        eprintln!("[WARNING] {} => {} ({} {})", s, p,
            score,
            levenshtein::levenshtein(p, &s[..n])
        );
    }

    score <= 0.125
}

struct MarkAsWatch<'a> {
    crunchyroll: &'a Crunchyroll,
    account_uuid: String,
    current_bearer_token: String,
    preferred_audio: String,
    locale: String
}

impl<'a> MarkAsWatch<'a> {
    async fn new(crunchyroll: &'a Crunchyroll,
        preferred_audio: Locale,
        locale: Locale
    ) -> Result<Self> {
        let account = crunchyroll.account().await?;
        let mut output = Self {
            crunchyroll: &crunchyroll,
            account_uuid: account.account_id,
            current_bearer_token: "".to_string(),
            preferred_audio: preferred_audio.to_string(),
            locale: locale.to_string(),
        };

        output.update_token().await?;
        Ok(output)
    }

    async fn update_token(&mut self) -> Result<()> {
        self.current_bearer_token = self.crunchyroll.access_token().await;
        Ok(())
    }

    async fn _mark_internal(&mut self, content_id: &String) -> Result<Response> {
        let query = self.crunchyroll.client().post(
            format!("https://www.crunchyroll.com/content/v2/discover/{}/mark_as_watched/{}?preferred_audio_language={}&locale={}",
                self.account_uuid,
                content_id,
                self.preferred_audio,
                self.locale
            )
        )
            .bearer_auth(&self.current_bearer_token)
            .build()?;
    
        Ok(self.crunchyroll.client()
            .execute(query)
            .await?
        )
    }

    async fn mark(&mut self, content_id: &String) -> Result<()> {
        let res = self._mark_internal(content_id).await?;
    
        if res.status().as_u16() == 401 {
            self.update_token().await?;

            self._mark_internal(content_id)
                .await?
                .error_for_status()?;
        } else {
            res.error_for_status()?;
        }
        Ok(())
    }
}

fn parse_date(x: &String) -> NaiveDate {
    let mut year: i32 = 0;
    let mut month: u32 = 0;
    let mut day: u32 = 0;

    let mut txt = x.chars();

    for c in &mut txt {
        if c.is_digit(10) {
            year = 10 * year + c.to_digit(10).unwrap() as i32;
            continue;
        }
        if c == '-' {
            break;
        }
        panic!("Invalid character in year {}", x);
    }

    for c in &mut txt {
        if c.is_digit(10) {
            month = 10 * month + c.to_digit(10).unwrap();
            continue;
        }
        if c == '-' {
            break;
        }
        panic!("Invalid character in month: {}", x);
    }

    for c in &mut txt {
        if c.is_digit(10) {
            day = 10 * day + c.to_digit(10).unwrap();
            continue;
        }
        panic!("Invalid character in day: {}", x);
    }

    NaiveDate::from_ymd_opt(year, month.max(1), day.max(1)).unwrap()
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let email = env::var("EMAIL")
        .expect("'EMAIL' environment variable not found");
    let password = env::var("PASSWORD")
        .expect("'PASSWORD' environment variable not found");

    let preferred_audio = Locale::from(
        env::var("PREFERRED_AUDIO")
            .expect("'PREFERRED_AUDIO' environment variable not found")
    );
    let locale = Locale::from(
        env::var("CLOCALE")
            .expect("'CLOCALE' environment variable not found")
    );
    
    let crunchyroll = Crunchyroll::builder()
        .preferred_audio_locale(preferred_audio.clone())
        .login_with_credentials(email, password)
        .await?;

    let mut mark_as_watcher = MarkAsWatch::new(
        &crunchyroll,
        preferred_audio,
        locale
    ).await?;

    let mut treated_ids = HashSet::<String>::new();
    let animes = read_mal_entries().await?;
    let max_date_difference = chrono::TimeDelta::days(2*30);

    for elt in animes {
        let (node, status) = (elt.node, elt.list_status);
        let air_start_date: Option<DateTime<Utc>> = 
            match node.start_date.as_ref() {
            None => None,
            Some(x) => {
                Utc.from_local_datetime(&NaiveDateTime::new(
                    parse_date(x),
                    NaiveTime::default()
                )).single()
            }
            };
        // We can do it, the status-less entries
        // have been filtered
        let status = status.unwrap();

        let title = get_node_title(node).to_lowercase();

        eprintln!("Querying {}", &title);
        let mut found = false;

        let mut query_result = crunchyroll.query(&title);
        if let Some(s) = query_result.series.next().await {
            let series = s?;
            eprintln!("Result '{}' '{}'", &series.title.to_lowercase(), &title);
    
            if same_title(&series.title.to_lowercase(), &title) {
                let seasons: Vec<crunchyroll_rs::Season> = series.seasons().await?;
                'SEASON: for season in seasons {
                    if treated_ids.contains(&season.id) {
                        continue;
                    }

                    if season.title.to_lowercase().as_str() != title.as_str() {
                        let mut valid_season = false;

                        if let Some(date) = air_start_date {
                            for episode in season.episodes().await? {
                                if (episode.episode_air_date - date).abs() < max_date_difference {
                                    valid_season = true;
                                    break;
                                }

                                if episode.episode_air_date >= (date+max_date_difference) {
                                    break 'SEASON;
                                }
                            }    
                        } else {
                            eprintln!("[WARNING] No date has been found");
                        }
                        
                        if !valid_season {
                            continue;
                        }
                    }

                    found = true;
                    eprintln!("Found {}", &season.title);
                    if status.num_episodes_watched == season.number_of_episodes {
                        match mark_as_watcher.mark(&season.id).await {
                        Ok(()) => (),
                        Err(e) => { dbg!(e); }
                        }
                    } else {
                        for episode in season.episodes().await? {
                            if let Some(episode_number) = episode.episode_number {
                                if episode_number > status.num_episodes_watched {
                                    continue;
                                }
                                if episode_number == 0 {
                                    // TODO: Check if this is necessary
                                    println!("Found an episode 0 for {}", &season.title);
                                    continue;
                                }
                            }
                            match mark_as_watcher.mark(&episode.id).await {
                            Ok(()) => (),
                            Err(e) => { dbg!(e); }
                            }
                        }    
                    }
                    treated_ids.insert(season.title);
                    break;
                }
            }
        }    

        if !found {
            println!("{}", title);
        }
    }

    Ok(())
}
