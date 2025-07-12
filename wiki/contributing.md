## Arnis Contribution Guide

This document outlines guidelines for contributing to the Arnis project. Arnis is a tool designed to convert OpenStreetMap (OSM) data into Minecraft worlds. Adhering to these guidelines helps maintain project quality and facilitates efficient collaboration.

### Contributing to Element Processing

Element processing involves the translation of OpenStreetMap tags and elements into Minecraft blocks and features. This is a core component of Arnis functionality.

* **Adding New Elements and Tags:** Contributions that extend Arnis's ability to process new OpenStreetMap elements and tags are welcome.
* **Tag Usage Requirement:** Before implementing processing for a new OSM tag, verify its global usage. The tag must be used at least **1,000 times** within OpenStreetMap to be considered for inclusion. This prevents the codebase from becoming burdened by niche tags with minimal global presence. Tag usage statistics can be verified on the [OSM Wiki](https://wiki.openstreetmap.org/wiki/Main_Page) or via [OSM Taginfo](https://taginfo.openstreetmap.org/).
* **Pull Request Submission:** All changes should be submitted via a Pull Request (PR). The PR description should clearly document the additions, specifying which new elements have been implemented and which OpenStreetMap tags are now supported.