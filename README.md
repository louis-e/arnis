<p align="center">
  <img width="456" height="125" src="https://github.com/louis-e/arnis/blob/main/gitassets/logo.png?raw=true">
</p>

# Arnis - Rust Edition (Development Branch)
This open source project generates any chosen location from the real world in Minecraft with a high level of detail.

⇒ [Where did you find this project?](https://6okq6xh5jt4.typeform.com/to/rSjZaB41)

---

This branch is dedicated to the ongoing effort to port the Arnis project from Python to Rust.

Run with the following arguments: ```cargo run --release -- --path="C:/YOUR_PATH/.minecraft/saves/worldname" --bbox="11.885287,48.292645,11.892861,48.295757"```

Key objectives of this port:
- **Modularity**: Ensure that all components (e.g., data fetching, processing, and world generation) are cleanly separated into distinct modules for better maintainability and scalability.
- **Cross-Platform Support**: Ensure the project runs smoothly on Windows, macOS, and Linux.
- **Comprehensive Documentation**: Detailed in-code documentation for a clear structure and logic.
- **User-Friendly Experience**: Focus on making the project easy to use for end users, with the potential to develop a graphical user interface (GUI) in the future. Suggestions and discussions on UI/UX are welcome.
- **Performance Optimization**: Utilize Rust’s memory safety and concurrency features to optimize the performance of the world generation process.


As of right now, pretty much all core features from the main Python branch are already ported to Rust. Please feel free to implement & fix, propose improvements, or suggest UI/UX enhancements. Contributions, discussions and suggestions are more than welcome.

Let's work together to build a high-performance, cross-platform, and user-friendly Rust version of Arnis!
<br>

## :desktop_computer: Example
<!---![Minecraft World Demo](https://github.com/louis-e/arnis/blob/main/gitassets/demo-comp.png?raw=true)
![Minecraft World Demo Before After](https://github.com/louis-e/arnis/blob/main/gitassets/before-after.gif?raw=true)--->
*To be updated*

## :floppy_disk: How it works
<!---![CLI Generation](https://github.com/louis-e/arnis/blob/main/gitassets/cli-generation.gif?raw=true)--->

The raw data obtained from the API *[(see FAQ)](#question-faq)* includes each element (buildings, walls, fountains, farmlands, etc.) with its respective corner coordinates (nodes) and descriptive tags. When you run Arnis, the following steps are performed automatically to generate a Minecraft world:

#### Processing Pipeline
<!---1. Scraping Data from API: The script fetches geospatial data from the Overpass Turbo API.
2. Determine Coordinate Extremes: Identifies the highest and lowest latitude and longitude values from the dataset.
3. Standardize Coordinate Lengths: Ensures all coordinates are of uniform length and removes the decimal separator.
4. Normalize Data: Adjusts all coordinates to start from zero by subtracting the previously determined lowest values.
5. Parse Data: Transforms the raw data into a standardized structure.
6. Sort elements by priority: Enables a layering system with prioritized elements.
7. Optimize Array Size: Focuses on the outermost buildings to reduce array size.
8. Generate Minecraft World: Iterates through the array to create the Minecraft world, including 3D structures like forests, houses, and rivers.--->
*To be updated*

## :keyboard: Usage
```cargo run --release -- --path="C:/YOUR_PATH/.minecraft/saves/worldname" --bbox="min_lng,min_lat,max_lng,max_lat"```

Use http://bboxfinder.com/ to draw a rectangle of your wanted area. Then copy the four box coordinates as shown below and use them as the input for the --bbox parameter.
![How to find area](https://github.com/louis-e/arnis/blob/main/gitassets/bbox-finder.png?raw=true)
The world will always be generated starting from the coordinates 0 0 0.

Manually generate a new Minecraft world (preferably a flat world) before running the script.
The --bbox parameter specifies the bounding box coordinates in the format: min_lng,min_lat,max_lng,max_lat.
Use --path to specify the location of the Minecraft world.
You can optionally use the parameter --debug to get a more detailed output during runtime.

## :question: FAQ
- *Wasn't this written in Python before?*<br>
Yes! Arnis was initially developed in Python, which benefited from Python's open-source friendliness and ease of readability. This is why we strive for clear, well-documented code in the Rust port of this project to find the right balance. I decided to port the project to Rust to learn more about it and push the algorithm's performance further. We were nearing the limits of optimization in Python, and Rust's capabilities allow for even better performance and efficiency.
- *Where does the data come from?*<br>
The geographic data is sourced from OpenStreetMap (OSM)[^1], a free, collaborative mapping project that serves as an open-source alternative to commercial mapping services. The data is accessed via the Overpass API, which queries OSM's database.
- *How does the Minecraft world generation work?*<br>
The script uses the [fastnbt](https://github.com/owengage/fastnbt) cargo package to interact with Minecraft's world format. This library allows Arnis to manipulate Minecraft region files, enabling the generation of real-world locations.
- *Where does the name come from?*<br>
The project is named after the smallest city in Germany, Arnis[^2]. The city's small size made it an ideal test case for developing and debugging the algorithm efficiently.

## :memo: ToDo and Known Bugs
Feel free to choose an item from the To-Do or Known Bugs list, or bring your own idea to the table. Bug reports shall be raised as a Github issue. Contributions are highly welcome and appreciated!
- [ ] Design and implement a GUI
- [ ] Fix faulty empty chunks ([https://github.com/owengage/fastnbt/issues/120](https://github.com/owengage/fastnbt/issues/120))
- [ ] Evaluate and implement multithreaded region saving
- [ ] Better code documentation
- [ ] Refactor railway implementation
- [ ] Refactor bridges implementation
- [ ] Refactor fountain structure implementation
- [ ] Tool for mapping real coordinates to Minecraft coordinates
- [ ] Setup fork of [https://github.com/aaronr/bboxfinder.com](https://github.com/aaronr/bboxfinder.com) for easy bbox picking
- [ ] Add interior to buildings
- [ ] Evaluate and implement elevation
- [ ] Generate a few big cities using high performance hardware and make them available to download

## :trophy: Hall of Fame Contributors
This section is dedicated to recognizing and celebrating the outstanding contributions of individuals who have significantly enhanced this project. Your work and dedication are deeply appreciated!

#### Contributors:
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
