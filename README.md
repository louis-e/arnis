<p align="center">
  <img width="456" height="125" src="https://github.com/louis-e/arnis/blob/main/gui-src/images/logo.png?raw=true">
</p>

# Arnis [![CI Build Status](https://github.com/louis-e/arnis/actions/workflows/ci-build.yml/badge.svg)](https://github.com/louis-e/arnis/actions) [<img alt="GitHub Release" src="https://img.shields.io/github/v/release/louis-e/arnis" />](https://github.com/louis-e/arnis/releases) [<img alt="GitHub Downloads (all assets, all releases" src="https://img.shields.io/github/downloads/louis-e/arnis/total" />](https://github.com/louis-e/arnis/releases)

Arnis creates complex and accurate Minecraft Java Edition worlds that reflect real-world geography and architecture using OpenStreetMap.

###### ⚠️ This Github page is the official project website. Do not download Arnis from any other website.

## :desktop_computer: Example
![Minecraft Preview](https://github.com/louis-e/arnis/blob/main/gitassets/mc.gif?raw=true)

Arnis is designed to handle large-scale data and generate rich, immersive environments that bring real-world cities, landmarks, and natural features into Minecraft. Whether you're looking to replicate your hometown, explore urban environments, or simply build something unique and realistic, Arnis generates your vision.

## :keyboard: Usage
<img width="60%" src="https://github.com/louis-e/arnis/blob/main/gitassets/gui.png?raw=true"><br>
Download the [latest release](https://github.com/louis-e/arnis/releases/) or [compile](#trophy-open-source) the project on your own.
 
Choose your area using the rectangle tool and select your Minecraft world - then simply click on 'Start Generation'!

> The world will always be generated starting from the Minecraft coordinates 0 0 0 (/tp 0 0 0). This is the top left of your selected area.
Minecraft version 1.16.5 and below is currently not supported, but we are working on it! For the best results, use Minecraft version 1.21.4 or above.
If you choose to select an own world, be aware that Arnis will overwrite certain areas.

[[Arch Linux AUR package](https://aur.archlinux.org/packages/arnis)]

## 📚 Documentation

Full documentation is available in the [GitHub Wiki](https://github.com/louis-e/arnis/wiki/), covering topics such as technical explanations, FAQs, contribution guidelines and roadmaps.

## :trophy: Open Source
#### Key objectives of this project
- **Modularity**: Ensure that all components (e.g., data fetching, processing, and world generation) are cleanly separated into distinct modules for better maintainability and scalability.
- **Performance Optimization**: We aim to keep a good performance and speed of the world generation process.
- **Comprehensive Documentation**: Detailed in-code documentation for a clear structure and logic.
- **User-Friendly Experience**: Focus on making the project easy to use for end users.
- **Cross-Platform Support**: We want this project to run smoothly on Windows, macOS, and Linux.

#### How to contribute
This project is open source and welcomes contributions from everyone! Whether you're interested in fixing bugs, improving performance, adding new features, or enhancing documentation, your input is valuable. Simply fork the repository, make your changes, and submit a pull request. Please respect the above mentioned key objectives. Contributions of all levels are appreciated, and your efforts help improve this tool for everyone.

Build and run it using: ```cargo run --release --no-default-features -- --path="C:/YOUR_PATH/.minecraft/saves/worldname" --bbox="min_lng,min_lat,max_lng,max_lat"```<br>
For the GUI: ```cargo run --release```<br>

> You can use the parameter --debug to get a more detailed output of the processed values, which can be helpful for debugging and development.

After your pull request was merged, I will take care of regularly creating update releases which will include your changes.

## :star: Star History

<a href="https://star-history.com/#louis-e/arnis&Date">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=louis-e/arnis&Date&theme=dark" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=louis-e/arnis&Date&type=Date" />
   <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=louis-e/arnis&Date&type=Date" />
 </picture>
</a>

## :copyright: License Information
Copyright (c) 2022-2025 Louis Erbkamm (louis-e)

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.[^3]

Download Arnis only from the official source (https://github.com/louis-e/arnis/). Every other website providing a download and claiming to be affiliated with the project is unofficial and may be malicious.

The logo was made by @nxfx21.


[^1]: https://en.wikipedia.org/wiki/OpenStreetMap

[^2]: https://en.wikipedia.org/wiki/Arnis,_Germany

[^3]: https://github.com/louis-e/arnis/blob/main/LICENSE
