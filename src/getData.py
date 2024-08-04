from random import choice
import json
import os
import requests
import subprocess


def download_with_requests(url, params, filename):
    response = requests.get(url, params=params)
    if response.status_code == 200:
        with open(filename, "w") as file:
            json.dump(response.json(), file)
        return filename
    else:
        print("Failed to download data. Status code:", response.status_code)
        return None


def download_with_curl(url, params, filename):
    # Prepare curl command with parameters
    curl_command = [
        "curl",
        "-o",
        filename,
        url + "?" + "&".join([f"{key}={value}" for key, value in params.items()]),
    ]
    subprocess.call(curl_command)
    return filename


def download_with_wget(url, params, filename):
    # Prepare wget command with parameters
    wget_command = [
        "wget",
        "-O",
        filename,
        url + "?" + "&".join([f"{key}={value}" for key, value in params.items()]),
    ]
    subprocess.call(wget_command)
    return filename


def getData(city, state, country, bbox, file, debug, download_method="requests"):
    print("Fetching data...")
    api_servers = [
        "https://overpass-api.de/api/interpreter",
        "https://lz4.overpass-api.de/api/interpreter",
        "https://z.overpass-api.de/api/interpreter",
        "https://overpass.kumi.systems/api/interpreter",
        "https://overpass.private.coffee/api/interpreter"
    ]
    url = choice(api_servers)

    if state:
        query1 = f"""
        [out:json];
        area[name="{city}"]->.city;
        area[name="{state}"]->.state;
        area[name="{country}"]->.country;
        (
            nwr(area.country)(area.state)(area.city)[building];
            nwr(area.country)(area.state)(area.city)[highway];
            nwr(area.country)(area.state)(area.city)[landuse];
            nwr(area.country)(area.state)(area.city)[natural];
            nwr(area.country)(area.state)(area.city)[leisure];
            nwr(area.country)(area.state)(area.city)[waterway]["waterway"!="fairway"];
            nwr(area.country)(area.state)(area.city)[amenity];
            nwr(area.country)(area.state)(area.city)[bridge];
            nwr(area.country)(area.state)(area.city)[railway];
            nwr(area.country)(area.state)(area.city)[barrier];
        );
        (._;>;);
        out;
        """
    elif bbox:
        bbox = bbox.split(",")
        bbox = [float(i) for i in bbox]
        if debug:
            print(f"Bbox input: {bbox}")
        
        query1 = f"""
        [out:json][bbox:{bbox[1]},{bbox[0]},{bbox[3]},{bbox[2]}];
        ( 
            nwr["building"];
            nwr["highway"];
            nwr["landuse"];
            nwr["natural"];
            nwr["leisure"];
            nwr["waterway"];
            nwr["amenity"];
            nwr["bridge"];
            nwr["railway"];
            nwr["barrier"];
        )->.waysinbbox;
        (
            node(w.waysinbbox);
        )->.nodesinbbox;
        .waysinbbox out body;
        .nodesinbbox out skel qt;
        """
    elif file:
        print("Loading data from file")
    else:
        query1 = f"""
        [out:json];
        area[name="{city}"]->.city;
        area[name="{country}"]->.country;
        (
            nwr(area.country)(area.city)[building];
            nwr(area.country)(area.city)[highway];
            nwr(area.country)(area.city)[landuse];
            nwr(area.country)(area.city)[natural];
            nwr(area.country)(area.city)[leisure];
            nwr(area.country)(area.city)[waterway]["waterway"!="fairway"];
            nwr(area.country)(area.city)[amenity];
            nwr(area.country)(area.city)[bridge];
            nwr(area.country)(area.city)[railway];
            nwr(area.country)(area.city)[barrier];
        );
        (._;>;);
        out;
        """

    if debug:
        print(f"OSM Query: {query1}")

    try:
        if file:
            with open("data.json", encoding="utf8") as dataset:
                data = json.load(dataset)
        else:
            if debug:
                print(f"Chosen server: {url}")
            filename = "arnis-debug-raw_data.json"
            if download_method == "requests":
                file_path = download_with_requests(url, {"data": query1}, filename)
            elif download_method == "curl":
                file_path = download_with_curl(url, {"data": query1}, filename)
            elif download_method == "wget":
                file_path = download_with_wget(url, {"data": query1}, filename)
            else:
                print("Invalid download method. Using 'requests' by default.")
                file_path = download_with_requests(url, {"data": query1}, filename)

            if file_path is None:
                os._exit(1)

            with open(file_path, "r") as file:
                data = json.load(file)

            if len(data["elements"]) == 0:
                print("Error! No data available")
                os._exit(1)

    except Exception as e:
        if "The server is probably too busy to handle your request." in str(e):
            print("Error! OSM server overloaded")
        elif "Dispatcher_Client::request_read_and_idx::rate_limited" in str(e):
            print("Error! IP rate limited, wait before trying again")
        else:
            print(f"Error! {e}")
        os._exit(1)

    return data
