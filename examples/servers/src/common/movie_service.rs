#![allow(dead_code)]

use std::sync::Arc;

use reqwest;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars::{self, JsonSchema},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JSON_Value;
use serde_json::json;
use std::result::Result;
use tokio::sync::Mutex;

use undrift_gps::gcj_to_wgs;

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GetCinemaListRequest {
    /// Current location latitude
    pub latitude: f64,
    /// Current location longitude
    pub longitude: f64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GetCinemaInformationRequest {
    /// Current city name
    pub cityname: String,
    /// Cinema ID
    pub cinema_id: i32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GetMovieDetailInfoRequest {
    /// movie ID
    pub movie_id: i32,
}

#[derive(Clone)]
pub struct Movie {
    client: reqwest::Client,
    city_id: Arc<Mutex<JSON_Value>>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Movie {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            city_id: Arc::new(Mutex::new(json!({}))),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Gets the current system time")]
    async fn get_current_time(&self) -> Result<CallToolResult, ErrorData> {
        let now = chrono::Local::now();
        let time_str = now.format("%Y-%m-%d %H:%M:%S").to_string();
        Ok(CallToolResult::success(vec![Content::text(time_str)]))
    }

    //List of nearby theaters
    #[tool(
        description = "Get a list of nearby movie theaters based on the latitude and longitude of the user's current location. It is not possible to obtain information on the latitude and longitude of the cinema here"
    )]
    async fn get_cinema_list(
        &self,
        Parameters(req): Parameters<GetCinemaListRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let cityname = match self
            .get_cityname_by_lat_lng(req.latitude, req.longitude)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[get_cinema_list] Failed to get city name: {:?}", e);
                return Err(ErrorData::invalid_request("Failed to get city name", None));
            }
        };

        let city_id = match self.get_city_id_by_cityname(cityname).await {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("[get_cinema_list] Failed to get city ID: {:?}", e);
                return Err(ErrorData::invalid_request("Failed to get city ID", None));
            }
        };

        //Build URL
        let url = format!(
            "https://apis.netstart.cn/maoyan/index/moreCinemas?day={}&offset={}&limit={}&districtId={}&lineId={}&hallType={}&brandId={}&serviceId={}&areaId={}&stationId={}&item&updateShowDay={}&reqId={}&cityId={}&lat={}&lng={}",
            "2025-6-9",
            "0",
            "5", //查询影院数量
            "-1",
            "-1",
            "-1",
            "-1",
            "-1",
            "-1",
            "-1",
            "ture",
            "1636710166221",
            city_id,
            req.latitude,
            req.longitude,
        );

        let response = match self.send_request(url).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[get_cinema_list] Failed to get cinema list: {:?}", e);
                return Err(ErrorData::invalid_request(
                    "Failed to get cinema list",
                    None,
                ));
            }
        };

        Ok(CallToolResult::success(vec![Content::text(response)]))
    }

    //Get theater details
    #[tool(
        description = "Get detailed information about the cinema and its movie schedule based on the cinema ID and city ID, including the latitude and longitude of the cinema, the schedule of the cinema, and more"
    )]
    async fn get_cinema_information(
        &self,
        Parameters(req): Parameters<GetCinemaInformationRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let city_id = match self.get_city_id_by_cityname(req.cityname).await {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("[get_cinema_information] Failed to get city ID: {:?}", e);
                return Err(ErrorData::invalid_request("Failed to get city ID", None));
            }
        };

        let cinema_info = match self.get_cinema_info(req.cinema_id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    "[get_cinema_information] Failed to get cinema info: {:?}",
                    e
                );
                return Err(ErrorData::invalid_request(
                    "Failed to get cinema info",
                    None,
                ));
            }
        };

        let mut cinema_json = match serde_json::from_str::<JSON_Value>(&cinema_info) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(
                    "[get_cinema_information] Failed to parse cinema JSON: {:?}",
                    e
                );
                return Err(ErrorData::invalid_request(
                    "Failed to parse cinema data",
                    None,
                ));
            }
        };

        let lat = match cinema_json["data"]["lat"].as_f64() {
            Some(a) => a,
            None => {
                tracing::error!("[get_cinema_information] Missing latitude in response");
                0.0
            }
        };
        let lng = match cinema_json["data"]["lng"].as_f64() {
            Some(a) => a,
            None => {
                tracing::error!("[get_cinema_information] Missing longitude in response");
                0.0
            }
        };
        let result = gcj_to_wgs(lat, lng);

        cinema_json["data"]["lat"] = json!(result.0);
        cinema_json["data"]["lng"] = json!(result.1);

        let new_cinema_info = match serde_json::to_string(&cinema_json) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("get text error,{:?}", e);
                return Err(ErrorData::invalid_request("get text error", None));
            }
        };

        let movie_info = match self.get_cinema_movie_info(req.cinema_id, city_id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[get_cinema_information] Failed to get movie info: {:?}", e);
                return Err(ErrorData::invalid_request("Failed to get movie info", None));
            }
        };

        Ok(CallToolResult::success(vec![
            Content::text(new_cinema_info),
            Content::text(movie_info),
        ]))
    }

    //Get movie information
    #[tool(description = "Get movie details based on the movie ID")]
    async fn get_movie_detail_info(
        &self,
        Parameters(req): Parameters<GetMovieDetailInfoRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let url = format!(
            "https://apis.netstart.cn/maoyan/movie/intro?movieId={}",
            req.movie_id
        );

        let movie_info = match self.send_request(url).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[get_movie_detail_info] Failed to get movie info: {:?}", e);
                return Err(ErrorData::invalid_request("Failed to get movie info", None));
            }
        };

        Ok(CallToolResult::success(vec![Content::text(movie_info)]))
    }
}

impl Movie {
    //Get city name based on latitude and longitude
    async fn get_cityname_by_lat_lng(
        &self,
        latitude: f64,
        longitude: f64,
    ) -> Result<String, ErrorData> {
        let url = format!(
            "https://apis.netstart.cn/maoyan/city/latlng?lat={}&lng={}",
            latitude, longitude
        );

        let text = match self.send_request(url).await {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("[get_cityname_by_lat_lng] Failed to get response: {:?}", e);
                return Err(ErrorData::invalid_request("Failed to get city data", None));
            }
        };

        let val = match serde_json::from_str::<JSON_Value>(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[get_cityname_by_lat_lng] Failed to parse JSON: {:?}", e);
                return Err(ErrorData::invalid_request(
                    "Failed to parse city data",
                    None,
                ));
            }
        };

        let city = match val["data"]["city"].as_str() {
            Some(a) => a.to_string(),
            None => {
                tracing::error!("[get_cityname_by_lat_lng] Missing city in response");
                return Err(ErrorData::invalid_request("Missing city data", None));
            }
        };

        Ok(city)
    }

    //Get information about the studio
    async fn get_cinema_info(&self, cinema_id: i32) -> Result<String, ErrorData> {
        let url = format!(
            "https://apis.netstart.cn/maoyan/cinema/detail?cinemaId={}",
            cinema_id
        );

        let response = match self.send_request(url).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[get_cinema_info] Failed to get cinema info: {:?}", e);
                return Err(ErrorData::invalid_request(
                    "Failed to get cinema info",
                    None,
                ));
            }
        };

        Ok(response)
    }

    //Get information about films shown in the studio
    async fn get_cinema_movie_info(
        &self,
        cinema_id: i32,
        city_id: i32,
    ) -> Result<String, ErrorData> {
        let url = format!(
            "https://apis.netstart.cn/maoyan/cinema/shows?cinemaId={}&ci={}&channelId=4",
            cinema_id, city_id
        );

        let response = match self.send_request(url).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[get_cinema_movie_info] Failed to get movie info: {:?}", e);
                return Err(ErrorData::invalid_request("Failed to get movie info", None));
            }
        };

        Ok(response)
    }

    //Send a GET request and return a string
    async fn send_request(&self, url: String) -> Result<String, ErrorData> {
        let response =match self.client.
        get(url).
        header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36").
        header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8").
        header("Accept-Language", "zh-CN,zh;q=0.9").
        send().
        await
        {
            Ok(r)=>r,
            Err(e)=>
            {
                tracing::error!("[send_request] Failed to send request: {:?}", e);
                return Err(ErrorData::invalid_request("Failed to send request", None));
            }
        };
        let result_text = match response.text().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[send_request] Failed to get response text: {:?}", e);
                return Err(ErrorData::invalid_request(
                    "Failed to get response text",
                    None,
                ));
            }
        };

        Ok(result_text)
    }

    async fn init_movie(&self) -> Result<bool, ErrorData> {
        let mut tmp = self.city_id.lock().await;
        *tmp = match self.get_all_city_id().await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("get response error,{:?}", e);
                return Err(ErrorData::invalid_request("response error", None));
            }
        };

        Ok(true)
    }

    //Get all city IDs
    async fn get_all_city_id(&self) -> Result<JSON_Value, ErrorData> {
        let url = "https://apis.netstart.cn/maoyan/cities.json";
        let response =match self.client.
        get(url).
        header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36").
        header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8").
        header("Accept-Language", "zh-CN,zh;q=0.9").
        send().
        await
        {
            Ok(r)=>r,
            Err(e)=>
            {
                tracing::error!("get response error,{:?}",e);
                return Err(ErrorData::invalid_request("response error",None));
            }
        };

        let result_json = match response.json::<JSON_Value>().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("get response error,{:?}", e);
                return Err(ErrorData::invalid_request("response error", None));
            }
        };

        Ok(result_json)
    }

    //Obtain the city ID based on the city name
    async fn get_city_id_by_cityname(&self, name: String) -> Result<i32, ErrorData> {
        let city_data = self.city_id.lock().await;

        let data: &Vec<JSON_Value> = city_data["cts"]
            .as_array()
            .ok_or_else(|| ErrorData::invalid_request("asdfasfe array is error", None))?;

        for city in data {
            // 获取城市名称
            let city_name = city["nm"]
                .as_str()
                .ok_or_else(|| ErrorData::invalid_request("data error", None))?;

            if name.contains(city_name) {
                // 找到匹配的城市，获取ID
                let city_id = city["id"]
                    .as_i64()
                    .ok_or_else(|| ErrorData::invalid_request("data error", None))?;

                return Ok(city_id as i32);
            }
        }

        Err(ErrorData::invalid_params("name is error", None))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Movie {
    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        let _ = self.init_movie().await;

        Ok(ServerHandler::get_info(self))
    }
}
