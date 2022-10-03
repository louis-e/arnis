# Arnis
*Arnis - Generate cities from real life in Minecraft using Python*<br>
Using this Python script, you can generate any city from the real world in Minecraft!<br><br>
YouTube Devlog: https://www.youtube.com/watch?v=5tOvtZG0_6k

## :desktop_computer: Example
![Minecraft World Demo](https://github.com/louis-e/arnis/blob/main/gitassets/demo-comp.png?raw=true)
![Minecraft World Demo Before After](https://github.com/louis-e/arnis/blob/main/gitassets/before-after.gif?raw=true)

## :floppy_disk: How it works
![CLI Generation](https://github.com/louis-e/arnis/blob/main/gitassets/cli-generation.gif?raw=true)

The raw data which we receive from the API *[(see FAQ)](#question-faq)* contains each element (buildings, walls, fountains, farmlands,...) with its respective corner coordinates as well as tags which describe the element.

When you run the script, the following steps are performed automatically:
1. Scraping data from API
2. Find biggest and lowest latitude and longitude coordinate value
3. Make the length of each coordinate the same and remove the coordinate separator dot
4. Normalize data to start from 0 by subtracting the beforehand determined lowest value from each coordinate
5. Parsing data into uniform structure
6. Iterate through each element and set the corresponding ID *[(see FAQ)](#question-faq)* at coordinate in array
7. Reduce array size by focusing on the outer buildings
8. Add up landuse layer and the other generated data
9. Iterate through array to generate Minecraft world

The last step is responsible for generating three dimensional structures like forests, houses or cemetery graves.

## :keyboard: Usage
```python3 arnis.py --city "Arnis" --state "Schleswig Holstein" --country "Deutschland" --path "C:/Users/username/AppData/Roaming/.minecraft/saves/worldname"```

Optional: ```--debug```
Notes:
- Manually generate a Minecraft world, preferably a flat world, before running the script.
- The city, state and country name should be in the local language of the respective country. Otherwise the city might not be found.
- You can optionally use the parameter ```--debug``` in order to see the processed values as a text output during runtime.

## :cd: Requirements
- Python 3
- At least 8 Gigabyte RAM memory

```pip install -r requirements.txt```

## :question: FAQ
- *Why do some cities take so long to generate?*<br>
There is a known problem with the floodfill algorithm, where big element outlines (e.g. farmlands,...) slow down the entire script for several seconds. When the geo data contains hundreds or even thousands of those big elements, it results in a long delay which can take up to multiple hours. Start with some small cities or towns before you generate bigger cities with the script in order to get a good feeling for how long it takes. I'm already thinking about a way to rewrite the floodfill algorithm as multithreaded to split up the task on multiple CPU cores which should reduce the delay drastically.
- *Where does the data come from?*<br>
In order to get the raw geo data, Arnis contacts a random Overpass Turbo API server. This API gets its data from the free collaborative geographic project OpenStreetMap (OSM)[^1]. OSM is an online community databse founded in 2004 which basically provides an open source Google Maps alternative.
- *Why can't my city be found?*<br>
Due to it being limited by the amount of user contribution, some areas might not be covered yet. See question above.
- *How does the Minecraft world generation work?*<br>
For this purpose I am using the [anvil-parser](https://github.com/matcool/anvil-parser) package.
- *Where does the name come from?*<br>
Arnis is the name of the smallest city in Germany[^2] and due to its size, I used it during development to debug the algorithm fast and efficiently. :shipit:
- *What are the corresponding IDs?*<br>
In step 6 *[(see How it works)](#floppy_disk-how-it-works)*, the script assigns an ID to each array element which is later translated and processed into a Minecraft world. These seperate steps are required to implement a layer system (e.g. farmlands should never overwrite buildings).

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
- [ ] Implement multithreading in floodfill algorithm in order to boost performance
- [ ] Add code comments
- [ ] Improve RAM usage
- [ ] Add interior to buildings
- [ ] Implement elevation
- [ ] Optimize region file size
- [ ] Street markings 
- [ ] Automated Tests

## :copyright: License
MIT License[^3]

Copyright (c) 2022 louis-e

[^1]: https://en.wikipedia.org/wiki/OpenStreetMap

[^2]: https://en.wikipedia.org/wiki/Arnis,_Germany

[^3]:
    Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
    
    The above copyright notice, the author ("louis-e") and this permission notice shall be included in all copies or substantial portions of the Software.
    
    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
