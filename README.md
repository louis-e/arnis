<p align="center">
  <img width="456" height="125" src="https://github.com/louis-e/arnis/blob/main/gitassets/logo.png?raw=true">
</p>

# Arnis [![Testing](https://github.com/louis-e/arnis/actions/workflows/python-app.yml/badge.svg)](https://github.com/louis-e/arnis/actions/workflows/python-app.yml)
This open source project generates any chosen location from the real world in Minecraft, allowing users to explore and build in a virtual world that mirrors the real one.
<br><br>
⇒ [Where did you find this project?](https://6okq6xh5jt4.typeform.com/to/rSjZaB41)
<br>
## :desktop_computer: Example
![Minecraft World Demo](https://github.com/louis-e/arnis/blob/main/gitassets/demo-comp.png?raw=true)
![Minecraft World Demo Before After](https://github.com/louis-e/arnis/blob/main/gitassets/before-after.gif?raw=true)

## :floppy_disk: How it works
![CLI Generation](https://github.com/louis-e/arnis/blob/main/gitassets/cli-generation.gif?raw=true)

The raw data obtained from the API *[(see FAQ)](#question-faq)* includes each element (buildings, walls, fountains, farmlands, etc.) with its respective corner coordinates (nodes) and descriptive tags. When you run the script, the following steps are performed automatically to generate a Minecraft world:

#### Processing Pipeline
1. Scraping Data from API: The script fetches geospatial data from the Overpass Turbo API.
2. Determine Coordinate Extremes: Identifies the highest and lowest latitude and longitude values from the dataset.
3. Standardize Coordinate Lengths: Ensures all coordinates are of uniform length and removes the decimal separator.
4. Normalize Data: Adjusts all coordinates to start from zero by subtracting the previously determined lowest values.
5. Parse Data: Transforms the raw data into a standardized structure.
6. Sort elements by priority: Enables a layering system with prioritized elements.
7. Optimize Array Size: Focuses on the outermost buildings to reduce array size.
8. Generate Minecraft World: Iterates through the array to create the Minecraft world, including 3D structures like forests, houses, and rivers.

## :keyboard: Usage
```python3 arnis.py --bbox="min_lng,min_lat,max_lng,max_lat" --path="C:/Users/username/AppData/Roaming/.minecraft/saves/worldname"```

Use http://bboxfinder.com/ to draw a rectangle of your wanted area. Then copy the four box coordinates as shown below and use them as the input for the --bbox parameter.
![How to find area](https://github.com/louis-e/arnis/blob/main/gitassets/bbox-finder.png?raw=true)
The world will always be generated starting from the coordinates 0 0 0.

Manually generate a new Minecraft world (preferably a flat world) before running the script.
The --bbox parameter specifies the bounding box coordinates in the format: min_lng,min_lat,max_lng,max_lat.
Use --path to specify the location of the Minecraft world.
With the --timeout parameter you can set the timeout for the floodfill algorithm in seconds (default: 2).
You can optionally use the parameter --debug to see processed value outputs during runtime.

#### Experimental City/State/Country Input Method
The following method is experimental and may not perform as expected. Support is limited.

```python3 arnis.py --city="CityName" --state="StateName" --country="CountryName" --path="C:/Users/username/AppData/Roaming/.minecraft/saves/worldname"```

### Docker image (experimental)
If you want to run this project containerized, you can use the Dockerfile provided in this repository. It will automatically scrape the latest source code from the repository. After running the container, you have to manually copy the generated region files from the container to the host machine in order to use them. When running the Docker image, set the ```--path``` parameter to ```/home```.
```
docker build -t arnis .
docker run arnis --city="Arnis" --state="Schleswig Holstein" --country="Deutschland" --path="/home"
docker cp CONTAINER_ID:/home/region DESTINATION_PATH
```

## :cd: Requirements
- Python 3
- ```pip install -r requirements.txt```

- To conform with style guide please format any changes and check the code quality
```black .``` 
```flake8 src/```

- Functionality should be covered by automated tests. 
```python -m pytest```

## :question: FAQ
- *Why do some cities take so long to generate?*<br>
The script's performance can be significantly affected by large elements, such as extensive farmlands. The floodfill algorithm can slow down considerably when dealing with such elements, leading to long processing times. Thus there is also a timeout restriction in place, which can be adjusted by the user *[(see Usage)](#keyboard-usage)*. It is recommended to start with smaller areas to get a sense of the script's performance. Continuous improvements on the algorithm especially focus on effiency improvements.
- *Where does the data come from?*<br>
The geographic data is sourced from OpenStreetMap (OSM)[^1], a free, collaborative mapping project that serves as an open-source alternative to commercial mapping services. The data is accessed via the Overpass API, which queries OSM's database.
- *How does the Minecraft world generation work?*<br>
The script uses the [anvil-parser](https://github.com/matcool/anvil-parser) library to interact with Minecraft's world format. This library allows the script to create and manipulate Minecraft region files, enabling the generation of real-world locations within the game.
- *Where does the name come from?*<br>
The project is named after Arnis[^2], the smallest city in Germany. The city's small size made it an ideal test case for developing and debugging the script efficiently.

## :memo: ToDo
Feel free to choose an item from the To-Do or Known Bugs list, or bring your own idea to the table. Contributions from everyone are welcome and encouraged to help improve this project.
- [ ] Look into https://github.com/Intergalactyc/anvil-new which seems to have a better support
- [ ] Tool for mapping real coordinates to Minecraft coordinates
- [ ] Fix railway orientation
- [ ] Fix gaps in bridges
- [ ] Full refactoring of variable and function names, establish naming conventions
- [ ] Detection of wrong bbox input
- [ ] Evaluate and implement multiprocessing in the ground layer initialization and floodfill algorithm
- [ ] Implement elevation
- [ ] Add interior to buildings
- [ ] Save fountain structure in the code (similar to the tree structure)
- [ ] Add windows to buildings
- [ ] Generate a few big cities using high performance hardware and make them available to download
- [ ] Optimize region file size
- [ ] Street markings
- [ ] Add better code comments
- [x] Alternative reliable city input options
- [x] Split up processData array into several smaller ones for big cities
- [x] Find alternative for CV2 package
- [x] Floodfill timeout parameter
- [x] Automated Tests
- [x] PEP8
- [x] Use f-Strings in print statements
- [x] Add Dockerfile
- [x] Added path check
- [x] Improve RAM usage

## :bug: Known Bugs
- [ ] Docker image size
- [x] 'Noer' bug (occurs when several different digits appear in coordinates before the decimal point)
- [x] 'Nortorf' bug (occurs when there are several elements with a big distance to each other, e.g. the API returns several different cities with the exact same name)
- [x] Saving step memory overflow
- [x] Non uniform OSM naming standards (dashes) (See name tags at https://overpass-turbo.eu/s/1mMj)

## :trophy: Hall of Fame Contributors
This section is dedicated to recognizing and celebrating the outstanding contributions of individuals who have significantly enhanced this project. Your work and dedication are deeply appreciated!

#### Contributors:
- callumfrance
- amir16yp
- EdwardWeir13579
- daniil2327

## :star: Star History

[![Star History Chart](https://api.star-history.com/svg?repos=louis-e/arnis&type=Date)](https://star-history.com/#louis-e/arnis&Date)

## :copyright: License Information
This project is licensed under the GNU General Public License v3.0 (GPL-3.0).[^3]

Copyright (c) 2022-2024 louis-e

[^1]: https://en.wikipedia.org/wiki/OpenStreetMap

[^2]: https://en.wikipedia.org/wiki/Arnis,_Germany

[^3]:
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
    For the full license text, see the LICENSE file.
