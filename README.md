<p align="center">
  <img width="456" height="125" src="https://github.com/louis-e/arnis/blob/main/gitassets/logo.png?raw=true">
</p>

# Arnis [![CI Build Status](https://github.com/louis-e/arnis/actions/workflows/ci-build.yml/badge.svg)](https://github.com/louis-e/arnis/actions)

This open source project written in Rust generates any chosen location from the real world in Minecraft with a high level of detail.

⇒ [Where did you find this project?](https://6okq6xh5jt4.typeform.com/to/rSjZaB41)

## :desktop_computer: Example
<img width="700" height="400" src="https://github.com/louis-e/arnis/blob/main/gitassets/mc.gif?raw=true">

By leveraging geospatial data from OpenStreetMap and utilizing the powerful capabilities of Rust, Arnis provides an efficient and robust solution for creating complex and accurate Minecraft worlds that reflect real-world geography and architecture.

Arnis is designed to handle large-scale data and generate rich, immersive environments that bring real-world cities, landmarks, and natural features into the Minecraft universe. Whether you're looking to replicate your hometown, explore urban environments, or simply build something unique and realistic, Arnis offers a comprehensive toolset to achieve your vision.

## :floppy_disk: How it works
![CLI Generation](https://github.com/louis-e/arnis/blob/main/gitassets/cli.gif?raw=true)

The raw data obtained from the API *[(see FAQ)](#question-faq)* includes each element (buildings, walls, fountains, farmlands, etc.) with its respective corner coordinates (nodes) and descriptive tags. When you run Arnis, the following steps are performed automatically to generate a Minecraft world:

#### Processing Pipeline
1. Fetch Data from Overpass API: The script retrieves geospatial data for the desired bounding box from the Overpass API. You can specify the bounding box coordinates using the --bbox parameter.
2. Parse Raw Data: The raw data is parsed to extract essential information like nodes, ways, and relations. Nodes are converted into Minecraft coordinates, and relations are handled similarly to ways, ensuring all relevant elements are processed correctly.
3. Prioritize and Sort Elements: The elements (nodes, ways, relations) are sorted by priority to establish a layering system, which ensures that certain types of elements (e.g., entrances and buildings) are generated in the correct order to avoid conflicts and overlapping structures.
4. Generate Minecraft World: The Minecraft world is generated using a series of element processors (generate_buildings, generate_highways, generate_landuse, etc.) that interpret the tags and nodes of each element to place the appropriate blocks in the Minecraft world. These processors handle the logic for creating 3D structures, roads, natural formations, and more, as specified by the processed data.
5. Generate Ground Layer: A ground layer is generated based on the provided scale factors to provide a base for the entire Minecraft world. This step ensures all areas have an appropriate foundation (e.g., grass and dirt layers).
6. Save the Minecraft World: All the modified chunks are saved back to the Minecraft region files.

## :keyboard: Usage
Get the [latest release](https://github.com/louis-e/arnis/releases/) or [compile](#trophy-open-source) the project on your own.
#### Run: ```arnis.exe --path="C:/YOUR_PATH/.minecraft/saves/worldname" --bbox="min_lng,min_lat,max_lng,max_lat"```

### How to find your bbox coordinates
Use http://bboxfinder.com/ to draw a rectangle of your wanted area. Then copy the four box coordinates as shown below and use them as the input for the --bbox parameter.
![How to find area](https://github.com/louis-e/arnis/blob/main/gitassets/bbox-finder.png?raw=true)
The world will always be generated starting from the coordinates 0 0 0.

Manually generate a new Minecraft world (preferably a flat world) before running the script.
The --bbox parameter specifies the bounding box coordinates in the format: min_lng,min_lat,max_lng,max_lat.
Use --path to specify the location of the Minecraft world.

## :question: FAQ
- *Wasn't this written in Python before?*<br>
Yes! Arnis was initially developed in Python, which benefited from Python's open-source friendliness and ease of readability. This is why we strive for clear, well-documented code in the Rust port of this project to find the right balance. I decided to port the project to Rust to learn more about it and push the algorithm's performance further. We were nearing the limits of optimization in Python, and Rust's capabilities allow for even better performance and efficiency. The old Python implementation is still available in the python-legacy branch.
- *Where does the data come from?*<br>
The geographic data is sourced from OpenStreetMap (OSM)[^1], a free, collaborative mapping project that serves as an open-source alternative to commercial mapping services. The data is accessed via the Overpass API, which queries OSM's database.
- *How does the Minecraft world generation work?*<br>
The script uses the [fastnbt](https://github.com/owengage/fastnbt) cargo package to interact with Minecraft's world format. This library allows Arnis to manipulate Minecraft region files, enabling the generation of real-world locations.
- *Where does the name come from?*<br>
The project is named after the smallest city in Germany, Arnis[^2]. The city's small size made it an ideal test case for developing and debugging the algorithm efficiently.

## :memo: ToDo and Known Bugs
Feel free to choose an item from the To-Do or Known Bugs list, or bring your own idea to the table. Bug reports shall be raised as a Github issue. Contributions are highly welcome and appreciated!
- [ ] Design and implement a GUI
- [ ] Evaluate and implement multithreaded region saving
- [ ] Better code documentation
- [ ] Implement house roof types
- [ ] Refactor railway implementation
- [ ] Refactor bridges implementation
- [ ] Refactor fountain structure implementation
- [ ] Automatic new world creation instead of using an existing world
- [ ] Tool for mapping real coordinates to Minecraft coordinates
- [ ] Setup fork of [https://github.com/aaronr/bboxfinder.com](https://github.com/aaronr/bboxfinder.com) for easy bbox picking
- [ ] Add interior to buildings
- [ ] Evaluate and implement elevation
- [ ] Generate a few big cities using high performance hardware and make them available to download
- [x] Fix faulty empty chunks ([https://github.com/owengage/fastnbt/issues/120](https://github.com/owengage/fastnbt/issues/120)) (workaround found)

## :trophy: Open Source
#### Key objectives of this project
- **Modularity**: Ensure that all components (e.g., data fetching, processing, and world generation) are cleanly separated into distinct modules for better maintainability and scalability.
- **Performance Optimization**: Utilize Rust’s memory safety and concurrency features to optimize the performance of the world generation process.
- **Comprehensive Documentation**: Detailed in-code documentation for a clear structure and logic.
- **User-Friendly Experience**: Focus on making the project easy to use for end users, with the potential to develop a graphical user interface (GUI) in the future. Suggestions and discussions on UI/UX are welcome.
- **Cross-Platform Support**: Ensure the project runs smoothly on Windows, macOS, and Linux.

#### How to contribute
This project is open source and welcomes contributions from everyone! Whether you're interested in fixing bugs, improving performance, adding new features, or enhancing documentation, your input is valuable. Simply fork the repository, make your changes, and submit a pull request. We encourage discussions and suggestions to ensure the project remains modular, optimized, and easy to use for the community. You can use the parameter --debug to get a more detailed output of the processed values, which can be helpful for debugging and development. Contributions of all levels are appreciated, and your efforts help improve this tool for everyone.

Build and run it using: ```cargo run --release -- --path="C:/YOUR_PATH/.minecraft/saves/worldname" --bbox="min_lng,min_lat,max_lng,max_lat"```<br>

After your pull request was merged, I will take care of regularly creating update releases which will include your changes.

#### Contributors:
This section is dedicated to recognizing and celebrating the outstanding contributions of individuals who have significantly enhanced this project. Your work and dedication are deeply appreciated!
- louis-e
- callumfrance
- amir16yp
- EdwardWeir13579
- daniil2327

*(Including original Python implementation)*

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
