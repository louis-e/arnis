# Technical Codebase Workflow

This page documents the internal flow of Arnis when a user initiates the "Start Generation" process. It includes backend component interactions, data flow, and transformation steps.

## Overview

The generation process starts from the frontend and passes through multiple Rust modules in the backend. The diagram below illustrates the full flow, including intermediate data stages and component responsibilities.

## Sequence Diagram
![image](https://github.com/user-attachments/assets/3713fb99-616e-4a36-82d0-35bf9c774f07)

```plantuml
@startuml
title Arnis World Generation Workflow (Technical)

actor User
participant "Frontend (Tauri/JS)" as Frontend
participant "gui.rs" as GUI
participant "retrieve_data.rs" as RetrieveData
participant "osm_parser.rs" as OSMParser
participant "ground.rs" as Ground
participant "map_transformation" as MapTransformation
participant "data_processing.rs" as DataProcessing
participant "progress.rs" as Progress
participant "fs" as Filesystem

User -> Frontend : Clicks "Start Generation"
Frontend -> GUI : invoke gui_start_generation(...)

alt New World & Spawn Point
    GUI -> GUI : update_player_position(...)\n- Validate spawn point in bbox\n- Write to level.dat
end

GUI -> RetrieveData : fetch_data_from_overpass(bbox, debug, "requests", None)
RetrieveData -> Progress : emit progress (fetching)
RetrieveData -> RetrieveData : Query Overpass API\nReturn OSM XML/JSON
RetrieveData -> GUI : Return raw_data

GUI -> OSMParser : parse_osm_data(raw_data, bbox, scale, debug)
OSMParser -> OSMParser : Parse nodes, ways, relations\nConvert to elements
OSMParser -> GUI : Return (elements, scale_x, scale_z)

GUI -> GUI : Sort elements by priority\n(landuse, buildings, etc.)

GUI -> Ground : generate_ground_data(args)
Ground -> GUI : Return ground

GUI -> MapTransformation : transform_map(elements, xzbbox, ground)
MapTransformation -> MapTransformation : Apply transformations (from JSON ops)
MapTransformation -> GUI : Return transformed elements

GUI -> DataProcessing : generate_world(elements, xzbbox, ground, args)
DataProcessing -> Progress : emit progress (generation)
DataProcessing -> DataProcessing : For each element:\n- Dispatch to element_processing::*\n- Place blocks via WorldEditor
DataProcessing -> Filesystem : Write region files, level.dat, etc.
DataProcessing -> Progress : emit progress (done)
DataProcessing -> GUI : Return success/failure

GUI -> Frontend : Return result (success/error)

note right of DataProcessing
Element processing includes:
- buildings.rs
- highways.rs
- landuse.rs
- water_areas.rs
- etc.
Each calls WorldEditor to place blocks.
end note
@enduml
