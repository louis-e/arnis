#!/usr/bin/env python

# Copyright 2022 by louis-e, https://github.com/louis-e/.
# MIT License
# Please see the LICENSE file that should have been included as part of this package.

import argparse
import gc
import numpy as np
import os
import sys

from .getData import getData
from .processData import processRawData, generateWorld

parser = argparse.ArgumentParser(
    description="Arnis - Generate cities from real life in Minecraft"
)
parser.add_argument("--bbox", dest="bbox", help="Bounding box of the area")
parser.add_argument("--city", dest="city", help="Name of the city (Experimental)")
parser.add_argument("--state", dest="state", help="Name of the state (Experimental)")
parser.add_argument(
    "--country", dest="country", help="Name of the country (Experimental)"
)
parser.add_argument("--file", dest="file", help="JSON file containing OSM data")
parser.add_argument(
    "--path", dest="path", required=True, help="Path to the minecraft world"
)
parser.add_argument(
    "--downloader",
    dest="downloader",
    choices=["requests", "curl", "wget"],
    default="requests",
    help="Downloader method (requests/curl/wget)",
)
parser.add_argument(
    "--debug",
    dest="debug",
    default=False,
    action="store_true",
    help="Enable debug mode",
)
parser.add_argument(
    "--timeout",
    dest="timeout",
    default=2,
    action="store_true",
    help="Set floodfill timeout (seconds)",
)
args = parser.parse_args()

# Ensure either bbox or city/state/country is provided
if not args.bbox and not (args.city and args.state and args.country):
    print(
        """Error! You must provide either a bounding box (bbox) or city/state/country \
(experimental) information."""
    )
    os._exit(1)

# Ensure file argument is handled correctly
if args.file and args.bbox:
    print("Error! You cannot provide both a bounding box (bbox) and a file.")
    os._exit(1)

gc.collect()
np.seterr(all="raise")
np.set_printoptions(threshold=sys.maxsize)

mcWorldPath = args.path
if mcWorldPath[-1] == "/":
    mcWorldPath = mcWorldPath[:-1]

def validate_bounding_box(bbox):
    """
    Validates a bounding box represented as a string in the format "min_lng,min_lat,max_lng,max_lat".

    Parameters:
        bbox (str): The bounding box string.

    Returns:
        bool: True if the bounding box is valid, False otherwise.
    """
    try:
        # Split the input string into components
        parts = bbox.split(',')
        if len(parts) != 4:
            return False

        # Convert the components to float
        min_lng, min_lat, max_lng, max_lat = map(float, parts)

        # Validate the longitude range (-180 to 180)
        if not (-180 <= min_lng <= 180 and -180 <= max_lng <= 180):
            return False

        # Validate the latitude range (-90 to 90)
        if not (-90 <= min_lat <= 90 and -90 <= max_lat <= 90):
            return False

        # Ensure that min_lng is less than max_lng and min_lat is less than max_lat
        if min_lng >= max_lng or min_lat >= max_lat:
            return False

        return True

    except ValueError:
        # In case of conversion error, input was not a valid float
        return False


def run():
    print(
        """\n
        ▄████████    ▄████████ ███▄▄▄▄    ▄█     ▄████████
        ███    ███   ███    ███ ███▀▀▀██▄ ███    ███    ███
        ███    ███   ███    ███ ███   ███ ███▌   ███    █▀
        ███    ███  ▄███▄▄▄▄██▀ ███   ███ ███▌   ███
      ▀███████████ ▀▀███▀▀▀▀▀   ███   ███ ███▌ ▀███████████
        ███    ███ ▀███████████ ███   ███ ███           ███
        ███    ███   ███    ███ ███   ███ ███     ▄█    ███
        ███    █▀    ███    ███  ▀█   █▀  █▀    ▄████████▀
                     ███    ███

                https://github.com/louis-e/arnis
          """
    )

    if not (os.path.exists(mcWorldPath + "/region")):
        print("Error! No Minecraft world found at given path")
        os._exit(1)

    if args.bbox:
        if validate_bounding_box(args.bbox) == False:
            print("Error! Invalid bbox input")
            os._exit(1)

    rawdata = getData(
        args.city,
        args.state,
        args.country,
        args.bbox,
        args.file,
        args.debug,
        args.downloader,
    )
    rawData = processRawData(rawdata, args)
    generateWorld(rawData)
    os._exit(0)
