var map, rsidebar, lsidebar, drawControl, drawnItems = null;

// Where we keep the big list of proj defs from the server
var proj4defs = null;
// Where we keep the proj objects we are using in this session
var projdefs = { "4326": L.CRS.EPSG4326, "3857": L.CRS.EPSG3857 };
var currentproj = "3857";
var currentmouse = L.latLng(0, 0);

/*
**
**  override L.Rectangle 
**  to fire an event after setting
**
**  the base parent object L.Path
**  includes the L.Mixin.Events
**
**  ensures bbox box is always
**  the topmost SVG feature
**
*/
L.Rectangle.prototype.setBounds = function (latLngBounds) {

    this.setLatLngs(this._boundsToLatLngs(latLngBounds));
    this.fire('bounds-set');
}


var FormatSniffer = (function () {  // execute immediately

    'use strict';

    /*
    **
    **  constructor
    **
    */
    var FormatSniffer = function (options) {

        options || (options = {});

        if (!this || !(this instanceof FormatSniffer)) {
            return new FormatSniffer(options);
        }


        this.regExes = {
            ogrinfoExtent: /Extent\:\s\((.*)\)/,
            bbox: /^\(([\s|\-|0-9]*\.[0-9]*,[\s|\-|0-9]*\.[0-9]*,[\s|\-|0-9]*\.[0-9]*,[\s|\-|0-9]*\.[0-9|\s]*)\)$/
        };
        this.data = options.data || "";
        this.parse_type = null;
    };

    /*
    **
    **  functions
    **
    */
    FormatSniffer.prototype.sniff = function () {
        return this._sniffFormat();
    };

    FormatSniffer.prototype._is_ogrinfo = function () {
        var match = this.regExes.ogrinfoExtent.exec(this.data.trim());
        var extent = [];
        if (match) {
            var pairs = match[1].split(") - (");
            for (var indx = 0; indx < pairs.length; indx++) {
                var coords = pairs[indx].trim().split(",");
                extent = (extent.concat([parseFloat(coords[0].trim()), parseFloat(coords[1].trim())]));
            }
        }
        this.parse_type = "ogrinfo";
        return extent;
    };

    FormatSniffer.prototype._is_normal_bbox = function () {
        var match = this.regExes.bbox.exec(this.data.trim());
        var extent = [];
        if (match) {
            var bbox = match[1].split(",");
            for (var indx = 0; indx < bbox.length; indx++) {
                var coord = bbox[indx].trim();
                extent = (extent.concat([parseFloat(coord)]));
            }
        }
        this.parse_type = "bbox";
        return extent;
    };

    FormatSniffer.prototype._is_geojson = function () {
        try {
            // try JSON
            var json = JSON.parse(this.data);

            // try GeoJSON
            var parsed_data = new L.geoJson(json)

        } catch (err) {

            return null;

        }

        this.parse_type = "geojson";
        return parsed_data;
    };

    FormatSniffer.prototype._is_wkt = function () {
        if (this.data === "") {
            throw new Error("empty -- nothing to parse");
        }

        try {
            var parsed_data = new Wkt.Wkt(this.data);
        } catch (err) {
            return null;
        }

        this.parse_type = "wkt";
        return parsed_data;
    };

    FormatSniffer.prototype._sniffFormat = function () {

        var parsed_data = null;
        var fail = false;
        try {
            var next = true;

            // try ogrinfo
            parsed_data = this._is_ogrinfo()
            if (parsed_data.length > 0) {
                next = false;
            }

            // try normal bbox 
            if (next) {
                parsed_data = this._is_normal_bbox();
                if (parsed_data.length > 0) next = false;
            }

            // try GeoJSON
            if (next) {
                parsed_data = this._is_geojson();
                if (parsed_data) next = false;
            }

            // try WKT
            if (next) {
                parsed_data = this._is_wkt();
                if (parsed_data) next = false;
            }

            // no matches, throw error
            if (next) {
                fail = true;
                /* 
                **  sorry, this block needs to be left aligned
                **  to make the alert more readable
                **  which means, we probably shouldn't use alerts ;-)
                */
                throw {
                    "name": "NoTypeMatchError",
                    "message": "The data is not a recognized format:\n \
1. ogrinfo extent output\n \
2. bbox as (xMin,yMin,xMax,yMax )\n \
3. GeoJSON\n \
4. WKT\n\n "
                }
            }


        } catch (err) {

            alert("Your paste is not parsable:\n" + err.message);
            fail = true;

        }

        // delegate to format handler
        if (!fail) {

            this._formatHandler[this.parse_type].call(this._formatHandler, parsed_data);

        }

        return (fail ? false : true);
    };


    /*
    **  an object with functions as property names.
    **  if we need to add another format
    **  we can do so here as a property name
    **  to enforce reusability
    **
    **  to add different formats as L.FeatureGroup layer 
    **  so they work with L.Draw edit and delete options
    **  we fake passing event information
    **  and triggering draw:created for L.Draw
    */
    FormatSniffer.prototype._formatHandler = {


        // coerce event objects to work with L.Draw types
        coerce: function (lyr, type_obj) {

            var event_obj = {
                layer: lyr,
                layerType: null,
            }

            // coerce to L.Draw types
            if (/point/i.test(type_obj)) {
                event_obj.layerType = "marker";
            }
            else if (/linestring/i.test(type_obj)) {
                event_obj.layerType = "polyline";
            }
            else if (/polygon/i.test(type_obj)) {
                event_obj.layerType = "polygon";
            }

            return event_obj;

        },

        reduce_layers: function (lyr) {
            var lyr_parts = [];
            if (typeof lyr['getLayers'] === 'undefined') {
                return [lyr];
            }
            else {
                var all_layers = lyr.getLayers();
                for (var i = 0; i < all_layers.length; i++) {
                    lyr_parts = lyr_parts.concat(this.reduce_layers(all_layers[i]));
                }
            }
            return lyr_parts;
        },

        get_leaflet_bounds: function (data) {
            /*
            **  data comes in an extent ( xMin,yMin,xMax,yMax )
            **  we need to swap lat/lng positions
            **  because leaflet likes it hard
            */
            var sw = [data[1], data[0]];
            var ne = [data[3], data[2]];
            return new L.LatLngBounds(sw, ne);
        },

        wkt: function (data) {
            var wkt_layer = data.construct[data.type].call(data);
            var all_layers = this.reduce_layers(wkt_layer);
            for (var indx = 0; indx < all_layers.length; indx++) {
                var lyr = all_layers[indx];
                var evt = this.coerce(lyr, data.type);

                // call L.Draw.Feature.prototype._fireCreatedEvent
                map.fire('draw:created', evt);
            }

        },

        geojson: function (geojson_layer) {
            var all_layers = this.reduce_layers(geojson_layer);
            for (var indx = 0; indx < all_layers.length; indx++) {
                var lyr = all_layers[indx];

                var geom_type = geojson_layer.getLayers()[0].feature.geometry.type;
                var evt = this.coerce(lyr, geom_type);

                // call L.Draw.Feature.prototype._fireCreatedEvent
                map.fire('draw:created', evt);
            }
        },

        ogrinfo: function (data) {
            var lBounds = this.get_leaflet_bounds(data);
            // create a rectangle layer
            var lyr = new L.Rectangle(lBounds);
            var evt = this.coerce(lyr, 'polygon');

            // call L.Draw.Feature.prototype._fireCreatedEvent
            map.fire('draw:created', evt);
        },

        bbox: function (data) {
            var lBounds = this.get_leaflet_bounds(data);
            // create a rectangle layer
            var lyr = new L.Rectangle(lBounds);
            var evt = this.coerce(lyr, 'polygon');

            // call L.Draw.Feature.prototype._fireCreatedEvent
            map.fire('draw:created', evt);
        }
    };

    return FormatSniffer; // return class def

})(); // end FormatSniffer


function addLayer(layer, name, title, zIndex, on) {
    if (on) {
        layer.setZIndex(zIndex).addTo(map);
    } else {
        layer.setZIndex(zIndex);
    }
    // Create a simple layer switcher that toggles layers on and off.
    var ui = document.getElementById('map-ui');
    var item = document.createElement('li');
    var link = document.createElement('a');
    link.href = '#';
    if (on) {
        link.className = 'enabled';
    } else {
        link.className = '';
    }
    link.innerHTML = name;
    link.title = title;
    link.onclick = function (e) {
        e.preventDefault();
        e.stopPropagation();

        if (map.hasLayer(layer)) {
            map.removeLayer(layer);
            this.className = '';
        } else {
            map.addLayer(layer);
            this.className = 'enabled';
        }
    };
    item.appendChild(link);
    ui.appendChild(item);
};

function formatBounds(bounds, proj) {
    var gdal = $("input[name='gdal-checkbox']").prop('checked');
    var lngLat = $("input[name='coord-order']").prop('checked');

    var formattedBounds = '';
    var southwest = bounds.getSouthWest();
    var northeast = bounds.getNorthEast();
    var xmin = 0;
    var ymin = 0;
    var xmax = 0;
    var ymax = 0;
    if (proj == '4326') {
        xmin = southwest.lng.toFixed(6);
        ymin = southwest.lat.toFixed(6);
        xmax = northeast.lng.toFixed(6);
        ymax = northeast.lat.toFixed(6);
    } else {
        var proj_to_use = null;
        if (typeof (projdefs[proj]) !== 'undefined') {
            // we have it already, then grab it and use it...
            proj_to_use = projdefs[proj];
        } else {
            // We have not used this one yet... make it and store it...
            projdefs[proj] = new L.Proj.CRS(proj, proj4defs[proj][1]);
            proj_to_use = projdefs[proj];
        }
        southwest = proj_to_use.project(southwest)
        northeast = proj_to_use.project(northeast)
        xmin = southwest.x.toFixed(4);
        ymin = southwest.y.toFixed(4);
        xmax = northeast.x.toFixed(4);
        ymax = northeast.y.toFixed(4);
    }

    if (gdal) {
        if (lngLat) {
            formattedBounds = xmin + ',' + ymin + ',' + xmax + ',' + ymax;
        } else {
            formattedBounds = ymin + ',' + xmin + ',' + ymax + ',' + xmax;
        }
    } else {
        if (lngLat) {
            formattedBounds = xmin + ' ' + ymin + ' ' + xmax + ' ' + ymax;
        } else {
            formattedBounds = ymin + ' ' + xmin + ' ' + ymax + ' ' + xmax;
        }
    }
    return formattedBounds
}

function formatTile(point, zoom) {
    var xTile = Math.floor((point.lng + 180) / 360 * Math.pow(2, zoom));
    var yTile = Math.floor((1 - Math.log(Math.tan(point.lat * Math.PI / 180) + 1 / Math.cos(point.lat * Math.PI / 180)) / Math.PI) / 2 * Math.pow(2, zoom));
    return xTile.toString() + ',' + yTile.toString();
}

function formatPoint(point, proj) {
    var gdal = $("input[name='gdal-checkbox']").prop('checked');
    var lngLat = $("input[name='coord-order']").prop('checked');

    var formattedPoint = '';
    if (proj == '4326') {
        x = point.lng.toFixed(6);
        y = point.lat.toFixed(6);
    } else {
        var proj_to_use = null;
        if (typeof (projdefs[proj]) !== 'undefined') {
            // we have it already, then grab it and use it...
            proj_to_use = projdefs[proj];
        } else {
            // We have not used this one yet... make it and store it...
            projdefs[proj] = new L.Proj.CRS(proj, proj4defs[proj][1]);
            proj_to_use = projdefs[proj];
        }
        point = proj_to_use.project(point)
        x = point.x.toFixed(4);
        y = point.y.toFixed(4);
    }
    if (gdal) {
        if (lngLat) {
            formattedBounds = x + ',' + y;
        } else {
            formattedBounds = y + ',' + x;
        }
    } else {
        if (lngLat) {
            formattedBounds = x + ' ' + y;
        } else {
            formattedBounds = y + ' ' + x;
        }
    }
    return formattedPoint
}

function validateStringAsBounds(bounds) {
    var splitBounds = bounds ? bounds.split(',') : null;
    return ((splitBounds !== null) &&
        (splitBounds.length == 4) &&
        ((-90.0 <= parseFloat(splitBounds[0]) <= 90.0) &&
            (-180.0 <= parseFloat(splitBounds[1]) <= 180.0) &&
            (-90.0 <= parseFloat(splitBounds[2]) <= 90.0) &&
            (-180.0 <= parseFloat(splitBounds[3]) <= 180.0)) &&
        (parseFloat(splitBounds[0]) < parseFloat(splitBounds[2]) &&
            parseFloat(splitBounds[1]) < parseFloat(splitBounds[3])))
}

$(document).ready(function () {
    /* 
    **
    **  make sure all textarea inputs
    **  are selected once they are clicked
    **  because some people might not 
    **  have flash enabled or installed
    **  and yes...
    **  there's a fucking Flash movie floating 
    **  on top of your DOM
    **
    */

    // init the projection input box as it is used to format the initial values
    $('input[type="textarea"]').on('click', function (evt) { this.select() });
    $("#projection").val(currentproj);

    // Initialize map
    map = L.map('map').setView([50.114768, 8.687322], 4);

    // Define available tile themes
    var tileThemes = {
        'osm': {
            url: 'https://tile.openstreetmap.org/{z}/{x}/{y}.png',
            options: {
                attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
                maxZoom: 19
            }
        },
        'esri-imagery': {
            url: 'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}',
            options: {
                attribution: 'Tiles &copy; Esri &mdash; Source: Esri, i-cubed, USDA, USGS, AEX, GeoEye, Getmapping, Aerogrid, IGN, IGP, UPR-EGP, and the GIS User Community',
                maxZoom: 18
            }
        },
        'opentopomap': {
            url: 'https://{s}.tile.opentopomap.org/{z}/{x}/{y}.png',
            options: {
                attribution: 'Map data: &copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors, <a href="http://viewfinderpanoramas.org">SRTM</a> | Map style: &copy; <a href="https://opentopomap.org">OpenTopoMap</a> (<a href="https://creativecommons.org/licenses/by-sa/3.0/">CC-BY-SA</a>)',
                maxZoom: 17
            }
        },
        'stadia-bright': {
            url: 'https://tiles.stadiamaps.com/tiles/alidade_smooth/{z}/{x}/{y}.{ext}',
            options: {
                minZoom: 0,
                maxZoom: 19,
                attribution: '&copy; <a href="https://www.stadiamaps.com/" target="_blank">Stadia Maps</a> &copy; <a href="https://openmaptiles.org/" target="_blank">OpenMapTiles</a> &copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
                ext: 'png'
            }
        },
        'stadia-dark': {
            url: 'https://tiles.stadiamaps.com/tiles/alidade_smooth_dark/{z}/{x}/{y}.{ext}',
            options: {
                minZoom: 0,
                maxZoom: 19,
                attribution: '&copy; <a href="https://www.stadiamaps.com/" target="_blank">Stadia Maps</a> &copy; <a href="https://openmaptiles.org/" target="_blank">OpenMapTiles</a> &copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
                ext: 'png'
            }
        }
    };

    // Global variable to store current tile layer
    var currentTileLayer = null;

    // Function to change tile theme with automatic HTTP fallback
    function changeTileTheme(themeKey) {
        // Remove current tile layer if it exists
        if (currentTileLayer) {
            map.removeLayer(currentTileLayer);
        }

        // Get the theme configuration
        var theme = tileThemes[themeKey];
        if (theme) {
            // Create and add new tile layer
            currentTileLayer = L.tileLayer(theme.url, theme.options);

            // Add automatic HTTP fallback for HTTPS failures
            var failureCount = 0;
            currentTileLayer.on('tileerror', function(error) {
                failureCount++;

                // After a few failures, try HTTP fallback
                if (failureCount >= 6 && !this._httpFallbackAttempted && theme.url.startsWith('https://')) {
                    console.log('HTTPS tile loading failed, attempting HTTP fallback for', themeKey);
                    this._httpFallbackAttempted = true;

                    // Create HTTP version of the URL
                    var httpUrl = theme.url.replace('https://', 'http://');

                    // Remove the failed HTTPS layer
                    map.removeLayer(this);

                    // Create new layer with HTTP URL
                    var httpLayer = L.tileLayer(httpUrl, theme.options);
                    httpLayer._httpFallbackAttempted = true;
                    httpLayer.addTo(map);
                    currentTileLayer = httpLayer;
                }
            });

            currentTileLayer.addTo(map);

            // Save preference to localStorage
            localStorage.setItem('selectedTileTheme', themeKey);
        }
    }

    // Load saved theme or default to OSM
    var savedTheme = localStorage.getItem('selectedTileTheme') || 'osm';
    changeTileTheme(savedTheme);

    // World overlay state
    var worldOverlay = null;
    var worldOverlayData = null;
    var worldOverlayEnabled = false;
    var worldPreviewAvailable = false;
    var sliderControl = null;

    // Create the opacity slider as a proper Leaflet control
    var SliderControl = L.Control.extend({
        options: { position: 'topleft' },
        onAdd: function(map) {
            var container = L.DomUtil.create('div', 'leaflet-bar world-preview-slider-container');
            container.id = 'world-preview-slider-container';
            container.style.display = 'none';

            var slider = L.DomUtil.create('input', 'world-preview-slider', container);
            slider.type = 'range';
            slider.min = '0';
            slider.max = '100';
            slider.value = '50';
            slider.id = 'world-preview-opacity';
            slider.title = 'Overlay Opacity';

            L.DomEvent.on(slider, 'input', function(e) {
                if (worldOverlay) {
                    worldOverlay.setOpacity(e.target.value / 100);
                }
            });

            // Prevent all map interactions
            L.DomEvent.disableClickPropagation(container);
            L.DomEvent.disableScrollPropagation(container);
            L.DomEvent.on(container, 'mousedown', L.DomEvent.stopPropagation);
            L.DomEvent.on(container, 'touchstart', L.DomEvent.stopPropagation);
            L.DomEvent.on(slider, 'mousedown', L.DomEvent.stopPropagation);
            L.DomEvent.on(slider, 'touchstart', L.DomEvent.stopPropagation);

            return container;
        }
    });

    // Function to add world preview button to the draw control's edit toolbar
    function addWorldPreviewToEditToolbar() {
        // Find the edit toolbar (contains Edit layers and Delete layers buttons)
        var editToolbar = document.querySelector('.leaflet-draw-toolbar:not(.leaflet-draw-toolbar-top)');
        if (!editToolbar) {
            // Try finding by the edit/delete buttons
            var deleteBtn = document.querySelector('.leaflet-draw-edit-remove');
            if (deleteBtn) {
                editToolbar = deleteBtn.parentElement;
            }
        }

        if (editToolbar) {
            // Create the preview button
            var toggleBtn = document.createElement('a');
            toggleBtn.className = 'leaflet-draw-edit-preview disabled';
            toggleBtn.href = '#';
            toggleBtn.title = 'Show World Preview (not available yet)';
            toggleBtn.id = 'world-preview-btn';

            toggleBtn.addEventListener('click', function(e) {
                e.preventDefault();
                e.stopPropagation();
                if (worldPreviewAvailable) {
                    toggleWorldOverlay();
                }
            });

            editToolbar.appendChild(toggleBtn);

            // Add the slider control to the map
            sliderControl = new SliderControl();
            map.addControl(sliderControl);
        }
    }

    // Toggle world overlay function
    function toggleWorldOverlay() {
        if (!worldPreviewAvailable || !worldOverlayData) return;

        worldOverlayEnabled = !worldOverlayEnabled;
        var btn = document.getElementById('world-preview-btn');
        var sliderContainer = document.getElementById('world-preview-slider-container');

        if (worldOverlayEnabled) {
            // Show overlay
            var data = worldOverlayData;
            var bounds = L.latLngBounds(
                [data.min_lat, data.min_lon],
                [data.max_lat, data.max_lon]
            );

            if (worldOverlay) {
                map.removeLayer(worldOverlay);
            }

            var opacity = document.getElementById('world-preview-opacity');
            var opacityValue = opacity ? opacity.value / 100 : 0.5;

            worldOverlay = L.imageOverlay(data.image_base64, bounds, {
                opacity: opacityValue,
                interactive: false,
                zIndex: 500
            });
            worldOverlay.addTo(map);

            if (btn) {
                btn.classList.add('active');
                btn.title = 'Hide World Preview';
            }
            if (sliderContainer) {
                sliderContainer.style.display = 'block';
            }
        } else {
            // Hide overlay
            if (worldOverlay) {
                map.removeLayer(worldOverlay);
                worldOverlay = null;
            }
            if (btn) {
                btn.classList.remove('active');
                btn.title = 'Show World Preview';
            }
            if (sliderContainer) {
                sliderContainer.style.display = 'none';
            }
        }
    }

    // Enable the preview button when data is available
    function enableWorldPreview(data) {
        worldOverlayData = data;
        worldPreviewAvailable = true;
        var btn = document.getElementById('world-preview-btn');
        if (btn) {
            btn.classList.remove('disabled');
            btn.title = 'Show World Preview';
        }
    }

    // Disable and reset preview (when world changes)
    function disableWorldPreview() {
        worldPreviewAvailable = false;
        worldOverlayData = null;
        worldOverlayEnabled = false;

        if (worldOverlay) {
            map.removeLayer(worldOverlay);
            worldOverlay = null;
        }

        var btn = document.getElementById('world-preview-btn');
        var sliderContainer = document.getElementById('world-preview-slider-container');
        if (btn) {
            btn.classList.add('disabled');
            btn.classList.remove('active');
            btn.title = 'Show World Preview (not available yet)';
        }
        if (sliderContainer) {
            sliderContainer.style.display = 'none';
        }
    }

    // Listen for messages from parent window
    window.addEventListener('message', function(event) {
        if (event.data && event.data.type === 'changeTileTheme') {
            changeTileTheme(event.data.theme);
        }

        // Handle world preview data ready (after generation completes)
        if (event.data && event.data.type === 'worldPreviewReady') {
            enableWorldPreview(event.data.data);

            // Auto-enable the overlay when generation completes
            if (!worldOverlayEnabled) {
                toggleWorldOverlay();
            }
        }

        // Handle existing world map load (zoom to location and auto-enable)
        if (event.data && event.data.type === 'loadExistingWorldMap') {
            var data = event.data.data;
            enableWorldPreview(data);

            // Calculate bounds and zoom to them
            var bounds = L.latLngBounds(
                [data.min_lat, data.min_lon],
                [data.max_lat, data.max_lon]
            );
            map.fitBounds(bounds, { padding: [50, 50] });

            // Auto-enable the overlay
            if (!worldOverlayEnabled) {
                toggleWorldOverlay();
            }
        }

        // Handle world changed (disable preview)
        if (event.data && event.data.type === 'worldChanged') {
            disableWorldPreview();
        }
    });

    // Set the dropdown value in parent window if it exists
    if (window.parent && window.parent.document) {
        var dropdown = window.parent.document.getElementById('tile-theme-select');
        if (dropdown) {
            dropdown.value = savedTheme;
        }
    }

    rsidebar = L.control.sidebar('rsidebar', {
        position: 'right',
        closeButton: true
    });
    rsidebar.on("sidebar-show", function (e) {
        $("#map .leaflet-tile-loaded").addClass("blurred");
    });
    rsidebar.on("sidebar-hide", function (e) {
        $('#map .leaflet-tile-loaded').removeClass('blurred');
        $('#map .leaflet-tile-loaded').addClass('unblurred');
        setTimeout(function () {
            $('#map .leaflet-tile-loaded').removeClass('unblurred');
        }, 7000);
    });

    map.addControl(rsidebar);

    lsidebar = L.control.sidebar('lsidebar', {
        position: 'left'
    });

    map.addControl(lsidebar);

    // Add in a crosshair for the map
    var crosshairIcon = L.icon({
        iconUrl: 'images/crosshair.png',
        iconSize: [20, 20], // size of the icon
        iconAnchor: [10, 10], // point of the icon which will correspond to marker's location
    });
    crosshair = new L.marker(map.getCenter(), { icon: crosshairIcon, clickable: false });
    crosshair.addTo(map);

    // Override default tooltips
    L.drawLocal = L.drawLocal || {};
    L.drawLocal.draw = L.drawLocal.draw || {};
    L.drawLocal.draw.toolbar = L.drawLocal.draw.toolbar || {};
    L.drawLocal.draw.toolbar.buttons = L.drawLocal.draw.toolbar.buttons || {};
    L.drawLocal.draw.toolbar.buttons.rectangle = 'Choose area';
    L.drawLocal.draw.toolbar.buttons.marker = 'Set spawnpoint';

    // Initialize the FeatureGroup to store editable layers
    drawnItems = new L.FeatureGroup();
    map.addLayer(drawnItems);

    // Custom icon for drawn markers
    var customMarkerIcon = L.icon({
        iconUrl: 'images/marker-icon.png',
        iconSize: [20, 20],
        iconAnchor: [10, 10],
        popupAnchor: [0, -10]
    });

    drawControl = new L.Control.Draw({
        edit: {
            featureGroup: drawnItems
        },
        draw: {
            rectangle: {
                shapeOptions: {
                    color: '#fe57a1',
                    opacity: 0.6,
                    weight: 3,
                    fillColor: '#fe57a1',
                    fillOpacity: 0.1,
                    dashArray: '10, 10',
                    lineCap: 'round',
                    lineJoin: 'round'
                },
                repeatMode: false
            },
            polyline: false,
            polygon: false,
            circle: false,
            marker: {
                icon: customMarkerIcon
            }
        }
    });
    map.addControl(drawControl);

    // Add world preview button to the edit toolbar after drawControl is added
    addWorldPreviewToEditToolbar();
    /*
    **
    **  create bounds layer
    **  and default it at first
    **  to draw on null island
    **  so it's not seen onload
    **
    */
    startBounds = new L.LatLngBounds([0.0, 0.0], [0.0, 0.0]);
    var bounds = new L.Rectangle(startBounds, {
        color: '#3778d4',
        opacity: 1.0,
        weight: 3,
        fill: '#3778d4',
        lineCap: 'round',
        lineJoin: 'round'
    });

    bounds.on('bounds-set', function (e) {
        // move it to the end of the parent if renderer exists
        if (e.target._renderer && e.target._renderer._container) {
            var parent = e.target._renderer._container.parentElement;
            $(parent).append(e.target._renderer._container);
        }

        // Set the hash
        var southwest = this.getBounds().getSouthWest();
        var northeast = this.getBounds().getNorthEast();
        var xmin = southwest.lng.toFixed(6);
        var ymin = southwest.lat.toFixed(6);
        var xmax = northeast.lng.toFixed(6);
        var ymax = northeast.lat.toFixed(6);
        location.hash = ymin + ',' + xmin + ',' + ymax + ',' + xmax;
    });
    map.addLayer(bounds);

    map.on('draw:created', function (e) {
        // If it's a marker, make sure we only have one
        if (e.layerType === 'marker') {
            // Remove any existing markers
            drawnItems.eachLayer(function(layer) {
                if (layer instanceof L.Marker) {
                    drawnItems.removeLayer(layer);
                }
            });
        }

        // Check if it's a rectangle and set proper styles before adding it to the layer
        if (e.layerType === 'rectangle') {
            e.layer.setStyle({
                color: '#3778d4',
                opacity: 1.0,
                weight: 3,
                fill: '#3778d4',
                lineCap: 'round',
                lineJoin: 'round'
            });
        }

        drawnItems.addLayer(e.layer);

        // Only update the bounds based on non-marker layers
        if (e.layerType !== 'marker') {
            // Calculate bounds only from non-marker layers
            const nonMarkerBounds = new L.LatLngBounds();
            let hasNonMarkerLayers = false;
            
            drawnItems.eachLayer(function(layer) {
                if (!(layer instanceof L.Marker)) {
                    hasNonMarkerLayers = true;
                    nonMarkerBounds.extend(layer.getBounds());
                }
            });
            
            // Only update bounds if there are non-marker layers
            if (hasNonMarkerLayers) {
                bounds.setBounds(nonMarkerBounds);
                $('#boxbounds').text(formatBounds(bounds.getBounds(), '4326'));
                $('#boxboundsmerc').text(formatBounds(bounds.getBounds(), currentproj));
                notifyBboxUpdate();
            }
        }

        if (!e.geojson &&
            !((drawnItems.getLayers().length == 1) && (drawnItems.getLayers()[0] instanceof L.Marker))) {
            map.fitBounds(bounds.getBounds());
        } else {
            if ((drawnItems.getLayers().length == 1) && (drawnItems.getLayers()[0] instanceof L.Marker)) {
                map.panTo(drawnItems.getLayers()[0].getLatLng());
            }
        }
    });

    map.on('draw:deleted', function (e) {
        e.layers.eachLayer(function (l) {
            drawnItems.removeLayer(l);
        });
        if (drawnItems.getLayers().length > 0 &&
            !((drawnItems.getLayers().length == 1) && (drawnItems.getLayers()[0] instanceof L.Marker))) {
            bounds.setBounds(drawnItems.getBounds())
            $('#boxbounds').text(formatBounds(bounds.getBounds(), '4326'));
            $('#boxboundsmerc').text(formatBounds(bounds.getBounds(), currentproj));
            notifyBboxUpdate();
            map.fitBounds(bounds.getBounds());
        } else {
            bounds.setBounds(new L.LatLngBounds([0.0, 0.0], [0.0, 0.0]));
            $('#boxbounds').text(formatBounds(bounds.getBounds(), '4326'));
            $('#boxboundsmerc').text(formatBounds(bounds.getBounds(), currentproj));
            notifyBboxUpdate();
            if (drawnItems.getLayers().length == 1) {
                map.panTo(drawnItems.getLayers()[0].getLatLng());
            }
        }
    });

    map.on('draw:edited', function (e) {
        // Calculate bounds only from non-marker layers
        const nonMarkerBounds = new L.LatLngBounds();
        let hasNonMarkerLayers = false;
        
        drawnItems.eachLayer(function(layer) {
            if (!(layer instanceof L.Marker)) {
                hasNonMarkerLayers = true;
                nonMarkerBounds.extend(layer.getBounds());
            }
        });
        
        // Only update bounds if there are non-marker layers
        if (hasNonMarkerLayers) {
            bounds.setBounds(nonMarkerBounds);
        }
        
        $('#boxbounds').text(formatBounds(bounds.getBounds(), '4326'));
        $('#boxboundsmerc').text(formatBounds(bounds.getBounds(), currentproj));
        notifyBboxUpdate();
        map.fitBounds(bounds.getBounds());
    });

    function display() {
        $('#boxbounds').text(formatBounds(bounds.getBounds(), '4326'));
        $('#boxboundsmerc').text(formatBounds(bounds.getBounds(), currentproj));
        notifyBboxUpdate();
    }
    display();

    map.on('move', function (e) {
        crosshair.setLatLng(map.getCenter());
    });

    // handle geolocation click events
    $('#geolocation').click(function () {
        map.locate({ setView: true, maxZoom: 8 });
        $('#geolocation a').toggleClass('active');
        $('#geolocation a').toggleClass('active', 350);
    });



    $('button#add').on('click', function (evt) {
        var sniffer = FormatSniffer({ data: $('div#rsidebar textarea').val() });
        var is_valid = sniffer.sniff();
        if (is_valid) {
            rsidebar.hide();
            $('#create-geojson a').toggleClass('enabled');
            map.fitBounds(bounds.getBounds());
        }
    });
    $('button#clear').on('click', function (evt) {
        $('div#rsidebar textarea').val('');
    });

    var initialBBox = location.hash ? location.hash.replace(/^#/, '') : null;
    if (initialBBox) {
        if (validateStringAsBounds(initialBBox)) {
            var splitBounds = initialBBox.split(',');
            startBounds = new L.LatLngBounds([splitBounds[0], splitBounds[1]],
                [splitBounds[2], splitBounds[3]]);
            var lyr = new L.Rectangle(startBounds, {
                color: '#3778d4',
                opacity: 1.0,
                weight: 3,
                fill: '#3778d4',
                lineCap: 'round',
                lineJoin: 'round'
            });
            var evt = {
                layer: lyr,
                layerType: "polygon",
            }
            map.fire('draw:created', evt);
            //map.fitBounds(bounds.getBounds());
        } else {
            // This will reset the hash if the original hash was not valid
            bounds.setBounds(bounds.getBounds());
        }
    } else {
        // Initially set the hash if there was not one set by the user
        bounds.setBounds(bounds.getBounds());
    }

    $("input").click(function (e) {
        display();
    });

});

function notifyBboxUpdate() {
    const bboxText = document.getElementById('boxbounds').textContent;
    window.parent.postMessage({ bboxText: bboxText }, '*');
}

// Expose marker coordinates to the parent window
function getSpawnPointCoords() {
    // Check if there are any markers in drawn items
    const markers = [];
    drawnItems.eachLayer(function(layer) {
        if (layer instanceof L.Marker) {
            const latLng = layer.getLatLng();
            markers.push({
                lat: latLng.lat,
                lng: latLng.lng
            });
        }
    });

    // Return the first marker found or null if none exists
    return markers.length > 0 ? markers[0] : null;
}

// Expose the function to the parent window
window.getSpawnPointCoords = getSpawnPointCoords;
