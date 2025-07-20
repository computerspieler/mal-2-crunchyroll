use dotenvy;
use mal_api::prelude::*;
use std::{io, thread, time::Duration};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    
    let mut username = String::new();
    eprintln!("Please enter your username:");
    io::stdin()
        .read_line(&mut username)
        .expect("Failed to read the username");

    let client_id = MalClientId::try_from_env().unwrap();
    let api_client = AnimeApiClient::from(&client_id);
    let common_fields = mal_api::anime::all_common_fields();

    let mut animes: Vec<AnimeListNode> = vec![];
    let max_page_size = 1000;
    let mut offset = 0;
    let mut done = false;

    while !done {
        eprintln!("Reading");
        thread::sleep(Duration::from_secs(2));
        let query = GetUserAnimeList::builder(username.as_str())
            .enable_nsfw()
            .offset(offset)
            .limit(max_page_size)
            .fields(&AnimeCommonFields(vec![AnimeField::list_status]))
            .build()
            .unwrap();
        let res = api_client.get_user_anime_list(&query).await;
        match res {
            Err(e) => {
                eprintln!("Error while retrieving the list: {}", e);
                done = true;
            }
            Ok(r) => {
                done = r.data.len() != (max_page_size as usize);
                animes.extend(r.data.into_iter());
            }
        }

        offset += max_page_size as u32;
    }

    eprintln!("{} elements read", animes.len());
    for elt in animes.into_iter() {
        let (node, status) = (elt.node, elt.list_status);
        if status.is_none() {
            continue;
        }
        let status = status.unwrap();
        if status.num_episodes_watched == 0 {
            continue;
        }

        
    }
}
