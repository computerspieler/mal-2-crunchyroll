use dotenvy;
use anyhow::Result;
use mal_api::prelude::*;
use crunchyroll_rs::{Crunchyroll, Locale};
use crunchyroll_rs::common::StreamExt;
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
                AnimeField::alternative_titles
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

fn is_prefix(p: &str, s: &str) -> bool {
    let n = p.len();
    if s.len() < n {
        return false;
    }
    return p == &s[..n];
}

struct MarkAsWatch<'a> {
    crunchyroll: &'a Crunchyroll,
    account_uuid: String,
    bearer_token_last_update: std::time::Instant,
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
            bearer_token_last_update: std::time::Instant::now(),
            preferred_audio: preferred_audio.to_string(),
            locale: locale.to_string(),
        };

        output.update_token().await?;
        Ok(output)
    }

    async fn update_token(&mut self) -> Result<()> {
        let now = std::time::Instant::now();
        if self.current_bearer_token.len() > 0 &&
            (now - self.bearer_token_last_update) < std::time::Duration::from_secs(5*60)
        {
            return Ok(());
        }

        self.current_bearer_token = self.crunchyroll.access_token().await;
        self.bearer_token_last_update = now;
        Ok(())
    }

    async fn mark(&mut self, content_id: &String) -> Result<()> {
        self.update_token().await?;

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
    
        self.crunchyroll.client()
            .execute(query)
            .await?
            .error_for_status()?;
        Ok(())
    }
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
    for elt in read_mal_entries().await? {
        let (node, status) = (elt.node, elt.list_status);
        // We can do it, the status-less entries
        // have been filtered
        let status = status.unwrap();

        let title = get_node_title(node).to_lowercase();
        eprintln!("Querying {}", &title);
        let mut found = false;

        let mut query_result = crunchyroll.query(&title);
        if let Some(s) = query_result.series.next().await {
            let series = s?;
    
            if is_prefix(&series.title.to_lowercase(), &title) {
                let seasons = series.seasons().await?;
                for season in seasons {
                    if treated_ids.contains(&season.id) {
                        continue;
                    }

                    if season.title.to_lowercase() != title {
                        continue;
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
                }
            }
        }    

        if !found {
            println!("{}", title);
        }
    }

    Ok(())
}
