import os
import requests
from random import choice


def getData(city, state, country, debug):
    print("Fetching data...")
    api_servers = [
        "https://overpass-api.de/api/interpreter",
        "https://lz4.overpass-api.de/api/interpreter",
        "https://z.overpass-api.de/api/interpreter",
        "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
        "https://overpass.openstreetmap.ru/api/interpreter",
        "https://overpass.kumi.systems/api/interpreter",
    ]
    url = choice(api_servers)
    query1 = (
        """
        [out:json];
        area[name="""
        + '"'
        + city.replace(" ", "-")
        + '"'
        + """]->.city;
        area[name="""
        + '"'
        + state.replace(" ", "-")
        + '"'
        + """]->.state;
        area[name="""
        + '"'
        + country
        + '"'
        + """]->.country;
        way(area.country)(area.state)(area.city)[!power][!place][!ferry];
        (._;>;);
        out;
    """
    )

    print(f"Chosen server: {url}")
    try:
        data = requests.get(url, params={"data": query1}).json()

        if len(data["elements"]) == 0:
            print("Error! No data available")
            os._exit(1)
    except Exception as e:
        if "The server is probably too busy to handle your request." in str(e):
            print("Error! OSM server overloaded")
        elif "Dispatcher_Client::request_read_and_idx::rate_limited" in str(e):
            print("Error! IP rate limited")
        else:
            print(f"Error! {e}")
        os._exit(1)

    if debug:
        with open("arnis-debug-raw_data.json", "w", encoding="utf-8") as f:
            f.write(str(data))
    return data
