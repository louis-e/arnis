<p align="center">
  <img width="456" height="125" src="https://github.com/louis-e/arnis/blob/main/gitassets/logo.png?raw=true">
</p>

# Arnis [![Testing](https://github.com/louis-e/arnis/actions/workflows/python-app.yml/badge.svg)](https://github.com/louis-e/arnis/actions/workflows/python-app.yml)
This open source project generates any chosen location from the real world in Minecraft, allowing users to explore and build in a virtual world that mirrors the real one.
<br>

## :desktop_computer: Example
![Minecraft World Demo](https://github.com/louis-e/arnis/blob/main/gitassets/demo-comp.png?raw=true)
![Minecraft World Demo Before After](https://github.com/louis-e/arnis/blob/main/gitassets/before-after.gif?raw=true)

## :floppy_disk: How it works
![CLI Generation](https://github.com/louis-e/arnis/blob/main/gitassets/cli-generation.gif?raw=true)

The raw data obtained from the API *[(see FAQ)](#question-faq)* includes each element (buildings, walls, fountains, farmlands, etc.) with its respective corner coordinates (nodes) and descriptive tags. When you run the script, the following steps are performed automatically to generate a Minecraft world:

1. Scraping Data from API: The script fetches geospatial data from the Overpass Turbo API.
2. Determine Coordinate Extremes: Identifies the highest and lowest latitude and longitude values from the dataset.
3. Standardize Coordinate Lengths: Ensures all coordinates are of uniform length and removes the decimal separator.
4. Normalize Data: Adjusts all coordinates to start from zero by subtracting the previously determined lowest values.
5. Parse Data: Transforms the raw data into a standardized structure.
6. Assign IDs to Elements: Iterates through each element and assigns a corresponding ID *[(see FAQ)](#question-faq)* at each coordinate in an array.
7. Optimize Array Size: Focuses on the outermost buildings to reduce array size.
8. Integrate Landuse and Additional Data: Combines the landuse layer with other generated data.
9. Generate Minecraft World: Iterates through the array to create the Minecraft world, including 3D structures like forests, houses, and cemetery graves.

## :keyboard: Usage
```python3 arnis.py --bbox="min_lng,min_lat,max_lng,max_lat" --path="C:/Users/username/AppData/Roaming/.minecraft/saves/worldname"```

Use http://bboxfinder.com/ to draw a rectangle of your wanted area. Then copy the four box coordinates as shown below and use them as the input for the --bbox parameter. 
![How to find area](https://github.com/louis-e/arnis/blob/main/gitassets/bbox-finder.png?raw=true)

Manually generate a new Minecraft world (preferably a flat world) before running the script.
The --bbox parameter specifies the bounding box coordinates in the format: min_lng,min_lat,max_lng,max_lat.
Use --path to specify the location of the Minecraft world.
You can optionally use the parameter --debug to see processed value outputs during runtime.

#### Experimental City/State/Country Input Method
The following method is experimental and may not perform as expected:

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

- To conform with style guide please format any changes
```black .``` 

- Functionality should be covered by automated tests. 
```python -m pytest```

## :question: FAQ
- *Why do some cities take so long to generate?*<br>
The script's performance can be significantly affected by large elements, such as extensive farmlands. The current floodfill algorithm can slow down considerably when dealing with such elements, leading to long processing times that can stretch to hours for large cities. Thus there is also a timeout restriction in place. It is recommended to start with smaller towns to get a sense of the script's performance. Future improvements will focus on making the floodfill algorithm multi-threaded to utilize multiple CPU cores and reduce processing times drastically.
- *Where does the data come from?*<br>
The geographic data is sourced from OpenStreetMap (OSM)[^1], a free, collaborative mapping project that serves as an open-source alternative to commercial mapping services. The data is accessed via the Overpass API, which queries OSM's database.
- *How does the Minecraft world generation work?*<br>
The script uses the [anvil-parser](https://github.com/matcool/anvil-parser) library to interact with Minecraft's world format. This library allows the script to create and manipulate Minecraft region files, enabling the generation of real-world locations within the game.
- *Where does the name come from?*<br>
The project is named after Arnis[^2], the smallest city in Germany. The city's small size made it an ideal test case for developing and debugging the script efficiently.
- *What are the corresponding IDs?*<br>
During the processing stage (*[(see How it works)](#floppy_disk-how-it-works)*), each element is assigned an ID that determines how it will be represented in Minecraft. This system ensures that different layers (e.g., buildings, roads, landuse) are rendered correctly without conflicts.

ID | Name | Note |
--- | --- | --- |
0 | Ground | |
10 | Street | |
11 | Footway | |
12 | Natural path | |
13 | Bridge | |
14 | Railway | |
19 | Street markings | Work in progress *[(see FAQ)](#question-faq)* |
20 | Parking | |
21 | Fountain border | |
22 | Fence | |
30 | Meadow | |
31 | Farmland | |
32 | Forest | |
33 | Cemetery | |
34 | Beach | |
35 | Wetland | |
36 | Pitch | |
37 | Swimming pool | |
38 | Water | |
39 | Raw grass | |
50-59 | House corner | The last digit refers to the building height |
60-69 | House wall | The last digit refers to the building height |
70-79 | House interior | The last digit refers to the building height |

## :memo: ToDo
Feel free to choose an item from the To-Do or Known Bugs list, or bring your own idea to the table. Contributions from everyone are welcome and encouraged to help improve this project.
- [x] Alternative reliable city input options[^3]
- [ ] Split up processData array into several smaller ones for big cities[^4]
- [ ] Floodfill timeout parameters
- [ ] Implement multiprocessing in floodfill algorithm in order to boost CPU bound calculation performance
- [ ] Generate a few big cities using high performance hardware and make them available to download[^3]
- [ ] Add code comments
- [ ] Implement elevation
- [ ] Find alternative for CV2 package
- [ ] Add interior to buildings
- [ ] Optimize region file size
- [ ] Street markings
- [x] Automated Tests
- [x] PEP8
- [x] Use f-Strings in print statements
- [x] Add Dockerfile
- [x] Added path check
- [x] Improve RAM usage

## :bug: Known Bugs
- [ ] 'Noer' bug (occurs when several different digits appear in coordinates before the decimal point)
- [x] 'Nortorf' bug (occurs when there are several elements with a big distance to each other, e.g. the API returns several different cities with the exact same name)
- [ ] Saving step memory overflow
- [ ] Docker image size
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

## :copyright: License
MIT License[^5]

Copyright (c) 2022 louis-e

[^1]: https://en.wikipedia.org/wiki/OpenStreetMap

[^2]: https://en.wikipedia.org/wiki/Arnis,_Germany

[^3]: This might require a complete redesign of the algorithm since the current version is based on the city / state / country input. This will be investigated further soon.

[^4]: https://github.com/louis-e/arnis/issues/21#issuecomment-1785801652

[^5]:
    Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
    
    The above copyright notice, the author ("louis-e") and this permission notice shall be included in all copies or substantial portions of the Software.
    
    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
