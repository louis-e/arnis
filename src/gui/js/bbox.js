var map, rsidebar, lsidebar, drawControl, drawnItems = null;

// logging.js defines this. The guard keeps a problem in the diagnostics path
// from turning into a broken map, which is the very failure mode it reports on.
if (typeof window.arnisLog !== 'function') {
    window.arnisLog = function (level, message) { console.log('[' + level + '] ' + message); };
}

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
**  inherits from L.Evented
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

// Interface text for the map. This page is an iframe of index.html (same
// origin) and the locale is loaded by the top document, so the strings are
// read through the parent; the English covers the moment before they arrive.
function mapText(key, fallback) {
    var loc = window.localization
        || (window.parent && window.parent !== window && window.parent.localization)
        || null;
    return (loc && typeof loc[key] === 'string' && loc[key]) || fallback;
}
window.mapText = mapText;

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
    map = L.map('map', { zoomControl: false }).setView([50.114768, 8.687322], 4);

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
        },
        'openfreemap-liberty': {
            type: 'vector',
            style: 'https://tiles.openfreemap.org/styles/liberty',
            attribution: '&copy; <a href="https://openfreemap.org" target="_blank">OpenFreeMap</a> &copy; <a href="https://www.openmaptiles.org/" target="_blank">OpenMapTiles</a> &copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'
        }
    };

    // Real orbital photography, served as Web Mercator XYZ, so the map CRS stays
    // EPSG:3857 and a body switch is just a layer swap. Row order differs between
    // the two and was verified: Mars is TMS, the CARTO-hosted Moon map is XYZ.
    var bodyBasemaps = {
        moon: {
            url: 'https://cartocdn-gusc.global.ssl.fastly.net/opmbuilder/api/v1/map/named/opm-moon-basemap-v0-1/all/{z}/{x}/{y}.png',
            options: {
                attribution: 'Basemap &copy; <a href="https://www.openplanetary.org/" target="_blank">OpenPlanetary</a> | LOLA/USGS | Elevation: NASA PDS LOLA',
                minZoom: 1,
                maxZoom: 12,
                maxNativeZoom: 10
            },
            // Copernicus crater: sharp rim, terraced walls, central peaks.
            home: [9.62, -20.08],
            homeZoom: 2
        },
        mars: {
            url: 'https://s3-eu-west-1.amazonaws.com/whereonmars.cartodb.net/viking_mdim21_global/{z}/{x}/{y}.png',
            options: {
                attribution: 'Basemap &copy; <a href="https://www.openplanetary.org/" target="_blank">OpenPlanetary</a> | NASA/Viking/USGS | Elevation: NASA PDS MOLA',
                tms: true,
                minZoom: 1,
                maxZoom: 12,
                maxNativeZoom: 7
            },
            // Valles Marineris: 7 km of relief and unmistakable from orbit.
            home: [-13.9, -59.2],
            homeZoom: 1
        }
    };

    // Global variable to store current tile layer
    var currentTileLayer = null;
    var currentBody = 'earth';
    // Read by updateTerrainPreviewButton, which lives outside this scope.
    window._currentBody = currentBody;

    /*
    **
    **  basemap failover
    **
    **  a basemap host the user's network cannot reach used to leave
    **  the map permanently blank: the only recovery was retrying the
    **  SAME host over plain HTTP, which cannot work for
    **  tile.openstreetmap.org (it 301s straight back to HTTPS) and is
    **  mixed-content blocked on macOS anyway. reported from mainland
    **  China and from networks where the OSM tile CDN refuses the
    **  client - see issues #1222, #1298, #1299.
    **
    **  so: walk a chain of independent operators until one paints.
    **  deliberately conservative - a single successful tile pins the
    **  provider for the session, so coverage gaps and zoom-limit 404s
    **  never cause a switch. only a provider that paints NOTHING is
    **  replaced.
    **
    */
    var BASEMAP_FALLBACK_CHAIN = [
        'osm',
        'esri-imagery',
        'opentopomap',
        'stadia-bright',
        'openfreemap-liberty'
    ];

    // Enough failures to rule out a couple of unlucky tiles, low enough to
    // recover well inside the watchdog when the host fails fast (DNS, TLS, 4xx).
    var BASEMAP_ERROR_THRESHOLD = 6;

    // Errors come back faster than images do, so a working provider can report
    // a handful of 404s before its first tile paints. Give every provider this
    // long to prove itself before the error count is allowed to condemn it; if
    // the errors stop arriving in the meantime, the stall watchdog still does.
    var BASEMAP_MIN_OBSERVE_MS = 3000;

    // Backstop for the failure the error counter cannot see: sockets that hang
    // rather than fail. That is the usual shape of a blocked host, and an <img>
    // can sit pending for a minute or more before it ever fires 'error'.
    var BASEMAP_STALL_MS = 12000;

    // The URL behind the 'custom' theme. It deliberately opts out of the
    // fallback chain: someone who picked Custom has already established that
    // the built-in hosts do not work for them, so quietly walking back through
    // those five would just be a slow route to the same blank map. Failures are
    // reported instead of papered over, which also means a typo in the template
    // is visible rather than hidden behind a provider that happens to load.
    var customTileUrl = (localStorage.getItem('customTileUrl') || '').trim();

    function isValidTileTemplate(url) {
        return /^https?:\/\//i.test(url) &&
            url.indexOf('{z}') !== -1 && url.indexOf('{x}') !== -1 && url.indexOf('{y}') !== -1;
    }

    // What is actually on screen, which is not necessarily what the user picked.
    var activeThemeKey = null;
    // Providers already ruled out in this recovery run, so it cannot cycle.
    var basemapAttempted = [];
    var basemapWatchdog = null;
    var basemapSettled = false;
    // A layer that has been removed can still deliver a queued tileload or
    // tileerror. Every health callback carries the generation it was armed in,
    // so a late event from the previous provider cannot settle or condemn the
    // one that replaced it.
    var basemapGeneration = 0;

    function clearBasemapWatchdog() {
        if (basemapWatchdog !== null) {
            clearTimeout(basemapWatchdog);
            basemapWatchdog = null;
        }
    }

    function basemapCurrent(generation) {
        return generation === basemapGeneration && !basemapSettled;
    }

    // First painted tile: this provider works, stop watching it.
    function markBasemapHealthy(generation) {
        if (!basemapCurrent(generation)) return;
        basemapSettled = true;
        clearBasemapWatchdog();
    }

    // Move to the next untried provider. Leaves selectedEarthTheme (the user's
    // preference) untouched, so their choice returns once the network does.
    function failBasemap(generation, reason) {
        if (!basemapCurrent(generation)) return;
        basemapSettled = true;
        clearBasemapWatchdog();

        var failed = activeThemeKey;
        var next = null;
        for (var i = 0; i < BASEMAP_FALLBACK_CHAIN.length; i++) {
            var candidate = BASEMAP_FALLBACK_CHAIN[i];
            if (tileThemes[candidate] && basemapAttempted.indexOf(candidate) === -1) {
                next = candidate;
                break;
            }
        }

        if (!next) {
            arnisLog('error', 'Basemap "' + failed + '" failed (' + reason +
                ') and every fallback provider has been tried. The map will stay blank; ' +
                'this network appears to block the tile hosts.');
            return;
        }

        arnisLog('warn', 'Basemap "' + failed + '" failed (' + reason +
            '), falling back to "' + next + '". User preference stays "' +
            selectedEarthTheme + '".');
        showEarthTheme(next);
    }

    // Records the Earth preference; off Earth the body's own basemap wins.
    function changeTileTheme(themeKey) {
        if (themeKey !== 'custom' && !tileThemes[themeKey]) return;
        selectedEarthTheme = themeKey;
        localStorage.setItem('selectedTileTheme', themeKey);
        if (currentBody === 'earth') applyBasemap();
    }

    function detachBasemap() {
        clearBasemapWatchdog();
        if (currentTileLayer) {
            map.removeLayer(currentTileLayer);
            currentTileLayer = null;
        }
    }

    // Mounts one Earth theme and arms its health checks. Only failBasemap and
    // applyBasemap call this; everything else goes through applyBasemap so the
    // recovery run is reset.
    function showEarthTheme(themeKey) {
        var theme = tileThemes[themeKey];
        if (!theme) return;

        detachBasemap();
        activeThemeKey = themeKey;
        basemapSettled = false;
        var generation = ++basemapGeneration;
        var mountedAt = Date.now();
        var lastFailureDetail = null;
        if (basemapAttempted.indexOf(themeKey) === -1) basemapAttempted.push(themeKey);

        if (theme.type === 'vector') {
            // Fall back to OSM raster if MapLibre plugin failed to load
            if (typeof L.maplibreGL !== 'function') {
                arnisLog('warn', 'MapLibre GL plugin unavailable, falling back to OSM raster');
                // Via changeTileTheme so the fallback is persisted, not retried next launch.
                changeTileTheme('osm');
                return;
            }
            currentTileLayer = L.maplibreGL({
                style: theme.style,
                attributionControl: { customAttribution: theme.attribution },
                pixelRatio: window.devicePixelRatio || 1
            });
            currentTileLayer.addTo(map);

            // The style document is a plain fetch: if the host is unreachable
            // the GL map reports an error and never reaches 'load'.
            var glMap = typeof currentTileLayer.getMaplibreMap === 'function'
                ? currentTileLayer.getMaplibreMap()
                : null;
            if (glMap) {
                glMap.on('load', function () { markBasemapHealthy(generation); });
                // Deliberately not a failure signal on its own: maplibre reports
                // a missing sprite or one bad tile the same way it reports an
                // unreachable style host. Remember the last one so the watchdog
                // can name a cause, and let the watchdog make the call.
                glMap.on('error', function (e) {
                    lastFailureDetail = (e && e.error && e.error.message) || 'style or tile request failed';
                });
            }
        } else {
            currentTileLayer = L.tileLayer(theme.url, theme.options);

            var errorCount = 0;
            currentTileLayer.on('tileload', function () { markBasemapHealthy(generation); });
            currentTileLayer.on('tileerror', function () {
                errorCount++;
                lastFailureDetail = errorCount + ' tile requests failed, none succeeded';
                if (errorCount >= BASEMAP_ERROR_THRESHOLD &&
                    Date.now() - mountedAt >= BASEMAP_MIN_OBSERVE_MS) {
                    failBasemap(generation, lastFailureDetail);
                }
            });

            currentTileLayer.addTo(map);
        }

        basemapWatchdog = setTimeout(function () {
            basemapWatchdog = null;
            failBasemap(generation, lastFailureDetail ||
                ('no tile painted within ' + (BASEMAP_STALL_MS / 1000) + 's'));
        }, BASEMAP_STALL_MS);
    }

    // Mounts the user's own tile source. No chain: see customTileUrl above.
    function showCustomBasemap(url) {
        detachBasemap();
        activeThemeKey = 'custom';
        basemapSettled = false;
        var generation = ++basemapGeneration;
        var mountedAt = Date.now();

        var host = url;
        try {
            host = new URL(url.replace(/\{[sxyz]\}/g, '0')).hostname;
        } catch (e) { /* keep the raw template for the attribution */ }

        currentTileLayer = L.tileLayer(url, {
            attribution: 'Tiles: ' + host,
            maxZoom: 19,
            // Only meaningful when the template uses {s}; harmless otherwise.
            subdomains: 'abc'
        });

        var errorCount = 0;
        function reportCustomFailure(reason) {
            if (generation !== basemapGeneration || basemapSettled) return;
            basemapSettled = true;
            clearBasemapWatchdog();
            arnisLog('error', 'Custom map source host "' + host + '" is not loading (' + reason +
                '). Check the URL template, or clear the setting to go back to the map themes.');
        }

        currentTileLayer.on('tileload', function () { markBasemapHealthy(generation); });
        currentTileLayer.on('tileerror', function () {
            if (generation !== basemapGeneration) return;
            errorCount++;
            if (errorCount >= BASEMAP_ERROR_THRESHOLD &&
                Date.now() - mountedAt >= BASEMAP_MIN_OBSERVE_MS) {
                reportCustomFailure(errorCount + ' tile requests failed, none succeeded');
            }
        });
        currentTileLayer.addTo(map);

        basemapWatchdog = setTimeout(function () {
            basemapWatchdog = null;
            reportCustomFailure('no tile painted within ' + (BASEMAP_STALL_MS / 1000) + 's');
        }, BASEMAP_STALL_MS);
    }

    // Driven by the settings field in the parent. An empty or unusable value
    // hands the map back to the theme chain.
    function setCustomTileUrl(url) {
        var next = (url || '').trim();
        if (next && !isValidTileTemplate(next)) {
            arnisLog('warn', 'Ignoring custom map source "' + next +
                '": expected an http(s) URL containing {z}, {x} and {y}.');
            next = '';
        }
        if (next === customTileUrl) return;
        customTileUrl = next;
        if (next) {
            localStorage.setItem('customTileUrl', next);
        } else {
            localStorage.removeItem('customTileUrl');
        }
        // Only redraws when the URL is the thing actually on screen.
        if (currentBody === 'earth' && selectedEarthTheme === 'custom') applyBasemap();
    }

    // Function to apply the active basemap, restarting the failover chain
    function applyBasemap() {
        basemapAttempted = [];

        if (currentBody !== 'earth') {
            // One source per body, so there is nothing to fall back to and the
            // watchdog would only fire pointlessly. Still worth a log line.
            detachBasemap();
            activeThemeKey = null;
            basemapSettled = true;
            // Same staleness rule as the Earth path: a removed layer can still
            // deliver queued tile events. Both the token and the body name are
            // captured, so a late Moon error cannot be reported against Mars.
            var bodyGeneration = ++basemapGeneration;
            var bodyName = currentBody;
            var body = bodyBasemaps[bodyName];
            currentTileLayer = L.tileLayer(body.url, body.options);
            var bodyErrors = 0;
            var bodyPainted = false;
            currentTileLayer.on('tileload', function () {
                if (bodyGeneration !== basemapGeneration) return;
                bodyPainted = true;
            });
            currentTileLayer.on('tileerror', function () {
                if (bodyGeneration !== basemapGeneration) return;
                bodyErrors++;
                if (!bodyPainted && bodyErrors === BASEMAP_ERROR_THRESHOLD) {
                    arnisLog('warn', 'Basemap for ' + bodyName + ' is not loading (' +
                        bodyErrors + ' failed tile requests, none succeeded).');
                }
            });
            currentTileLayer.addTo(map);
            return;
        }

        if (selectedEarthTheme === 'custom') {
            if (isValidTileTemplate(customTileUrl)) {
                showCustomBasemap(customTileUrl);
            } else if (!currentTileLayer) {
                // Custom picked but nothing usable entered yet. Show a real map
                // rather than a blank one, without overwriting their choice.
                showEarthTheme(BASEMAP_FALLBACK_CHAIN[0]);
            }
            return;
        }

        showEarthTheme(selectedEarthTheme);
    }

    // Load saved theme or default to OSM
    var savedTheme = localStorage.getItem('selectedTileTheme') || 'osm';
    var selectedEarthTheme = savedTheme;

    var BODY_CYCLE = ['earth', 'moon', 'mars'];
    var _bodyToggleBtn = null;

    // Icon and tooltip carry the whole state: which world is selected, and
    // (in the tooltip) its block size off Earth.
    function syncBodyToggleButton() {
        if (!_bodyToggleBtn) return;
        BODY_CYCLE.forEach(function (b) {
            _bodyToggleBtn.classList.toggle('body-' + b, b === currentBody);
        });
        refreshToolLabel(_bodyToggleBtn);
    }

    // Driven by the world toggle in the map toolbar. Earth is restored on every
    // start, so a Moon world stays a deliberate choice.
    function changeBody(body) {
        if (body === currentBody) return;
        currentBody = bodyBasemaps[body] ? body : 'earth';
        window._currentBody = currentBody;

        // A bbox from another body is meaningless. The existing delete path also
        // resets bounds to the 0,0,0,0 sentinel the parent watches for.
        if (drawnItems && drawnItems.getLayers().length) {
            var removed = L.layerGroup();
            drawnItems.eachLayer(function (l) { removed.addLayer(l); });
            map.fire('draw:deleted', { layers: removed });
        }

        // The preview is an image overlay pinned to lat/lng bounds on the body
        // it was generated for, so leaving it up would drape a stretch of Earth
        // (or the previous body) over the new basemap at coordinates that mean
        // something else entirely.
        disableWorldPreview();

        applyBasemap();

        if (currentBody === 'earth') {
            map.setView([50.114768, 8.687322], 4);
        } else {
            var b = bodyBasemaps[currentBody];
            map.setView(b.home, b.homeZoom);
        }

        // Nominatim only knows Earth place names.
        var search = document.getElementById('search-container');
        if (search) search.style.display = currentBody === 'earth' ? '' : 'none';

        syncBodyToggleButton();
        // Earth-only, so its tooltip and enabled state change with the body.
        updateTerrainPreviewButton();
    }

    applyBasemap();

    // World overlay state. For a One World the data carries `areas` and the
    // overlay is a layer group.
    var worldOverlay = null;
    var worldOverlayData = null;
    var worldOverlayEnabled = false;
    var worldPreviewAvailable = false;
    var sliderControl = null;
    // One World: the user hid the areas, and the world they were hidden for.
    var oneWorldOverlayHidden = false;
    // Slider positions per kind: the opacity (0..100; a One World opaque by default), and with
    // Advanced features the balance (-100..100, 0 = both layers at their default).
    var overlayOpacity = { world: 70, oneWorld: 100 };
    var overlayBalance = { world: 0, oneWorld: 0 };
    // On the map-first page the world's preview images draw under the selection and the piece
    // outlines (overlayPane is 400); Classic keeps them in overlayPane.
    map.createPane('arnisPreview').style.zIndex = 350;
    // The cell overlay (its darkening, outlines and fills) and its numbers each get a pane, so the
    // balance slider can fade them as one. The outlines sit just under the selection's pane.
    var cellsPane = map.createPane('arnisCells');
    cellsPane.style.zIndex = 399;
    var cellLabelsPane = map.createPane('arnisCellLabels');
    cellLabelsPane.style.zIndex = 640;
    var previewPane = function () { return window.arnisMapFirst ? 'arnisPreview' : 'overlayPane'; };
    var oneWorldFittedPath = null;

    function setWorldOverlayOpacity(value) {
        if (!worldOverlay) return;
        if (typeof worldOverlay.setOpacity === 'function') {
            worldOverlay.setOpacity(value);
            return;
        }
        worldOverlay.eachLayer(function (layer) {
            if (typeof layer.setOpacity === 'function') {
                layer.setOpacity(value);
            } else if (typeof layer.setStyle === 'function') {
                layer.setStyle({
                    opacity: Math.min(1, value + 0.3),
                    fillOpacity: layer.options.arnisFill ? value * 0.15 : 0
                });
            }
        });
    }

    function buildWorldOverlay(data, opacityValue) {
        if (!data.areas) {
            var bounds = L.latLngBounds([data.min_lat, data.min_lon], [data.max_lat, data.max_lon]);
            return L.imageOverlay(data.image_base64, bounds, {
                pane: previewPane(),
                opacity: opacityValue,
                interactive: false,
                zIndex: 500
            });
        }
        var group = L.layerGroup();
        data.areas.forEach(function (area, index) {
            var b = L.latLngBounds([area.min_lat, area.min_lon], [area.max_lat, area.max_lon]);
            if (area.image_base64) {
                group.addLayer(L.imageOverlay(area.image_base64, b, {
                    pane: previewPane(),
                    opacity: opacityValue,
                    interactive: false,
                    zIndex: 500 + index
                }));
            }
            // Filled only where there is no image to show.
            group.addLayer(L.rectangle(b, {
                color: '#fecc44',
                weight: 1,
                opacity: Math.min(1, opacityValue + 0.3),
                fillColor: '#fecc44',
                fillOpacity: area.image_base64 ? 0 : opacityValue * 0.15,
                arnisFill: !area.image_base64,
                interactive: false
            }));
        });
        return group;
    }

    function oneWorldBounds(areas) {
        var bounds = null;
        areas.forEach(function (area) {
            var b = L.latLngBounds([area.min_lat, area.min_lon], [area.max_lat, area.max_lon]);
            bounds = bounds ? bounds.extend(b) : b;
        });
        return bounds;
    }

    // The world overlay's own default opacity: 70 %, or opaque for a One World in Classic.
    function worldBaseOpacity() {
        return worldOverlayData && worldOverlayData.areas && !window.arnisMapFirst ? 1 : 0.7;
    }

    function balanceValue() {
        var slider = document.getElementById('world-preview-opacity');
        return slider ? Number(slider.value) : 0;
    }

    // The slider is the balance with Advanced features on, the plain opacity otherwise.
    function balanceOn() {
        return Boolean(window.arnisMeldOn);
    }

    function sliderKind() {
        return worldOverlayData && worldOverlayData.areas ? 'oneWorld' : 'world';
    }

    function worldOpacity() {
        return balanceOn() ? window.arnisBalance(balanceValue(), worldBaseOpacity()).world : balanceValue() / 100;
    }

    // Fade the world overlay (left of centre) or the cell overlay (right of centre).
    function applyBalance() {
        var cells = balanceOn() ? window.arnisBalance(balanceValue(), worldBaseOpacity()).cells : 1;
        setWorldOverlayOpacity(worldOpacity());
        cellsPane.style.opacity = cellLabelsPane.style.opacity = String(cells);
        cellsPane.style.display = cellLabelsPane.style.display = cells <= 0 ? 'none' : '';
    }

    // The slider shows while the world overlay (or, as the balance, the cells) is on the map.
    function updateSliderVisibility() {
        var box = document.getElementById('world-preview-slider-container');
        if (!box) return;
        var balance = balanceOn();
        box.style.display = !(worldOverlay || (balance && lastPieces.length)) ? 'none' : balance ? 'flex' : 'block';
        // Read each time, so a language change reaches it.
        box.title = balance ? mapText('map_balance_tip', 'Left fades the world tiles, right fades the cells. Double-click to reset.') : '';
    }

    // The Advanced features switch turns the slider into the balance and back.
    function syncSliderMode() {
        var slider = document.getElementById('world-preview-opacity');
        var box = document.getElementById('world-preview-slider-container');
        if (!slider || !box) return;
        var balance = balanceOn();
        box.classList.toggle('is-balance', balance);
        slider.min = balance ? '-100' : '0';
        slider.title = balance ? '' : 'Overlay Opacity';
        slider.value = String((balance ? overlayBalance : overlayOpacity)[sliderKind()]);
        applyBalance();
        updateSliderVisibility();
    }

    // Create the opacity slider as a proper Leaflet control
    var SliderControl = L.Control.extend({
        options: { position: 'topleft' },
        onAdd: function(map) {
            var container = L.DomUtil.create('div', 'leaflet-bar world-preview-slider-container');
            container.id = 'world-preview-slider-container';
            container.style.display = 'none';

            // Tiles on the left, the grid on the right: what each end fades.
            var icon = function (paths) {
                var span = L.DomUtil.create('span', 'world-preview-slider-icon', container);
                span.innerHTML = '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true">' + paths + '</svg>';
                return span;
            };
            icon('<rect x="2" y="2" width="5" height="5" fill="currentColor" stroke="none"/><rect x="9" y="2" width="5" height="5" fill="currentColor" stroke="none" opacity=".5"/><rect x="2" y="9" width="5" height="5" fill="currentColor" stroke="none" opacity=".5"/><rect x="9" y="9" width="5" height="5" fill="currentColor" stroke="none"/>');
            var track = L.DomUtil.create('span', 'world-preview-slider-track', container);
            var slider = L.DomUtil.create('input', 'world-preview-slider', track);
            slider.type = 'range';
            slider.min = '0';
            slider.max = '100';
            slider.value = '70';
            slider.id = 'world-preview-opacity';
            slider.title = 'Overlay Opacity';
            L.DomUtil.create('i', 'world-preview-slider-tick', track);
            icon('<rect x="2" y="2" width="12" height="12"/><path d="M8 2v12M2 8h12"/>');

            L.DomEvent.on(slider, 'input', function(e) {
                (balanceOn() ? overlayBalance : overlayOpacity)[sliderKind()] = Number(e.target.value);
                applyBalance();
            });
            // Double-click puts the balance back at the centre.
            L.DomEvent.on(slider, 'dblclick', function() {
                if (!balanceOn()) return;
                slider.value = '0';
                slider.dispatchEvent(new Event('input'));
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
            toggleBtn.id = 'world-preview-btn';

            toggleBtn.addEventListener('click', function(e) {
                e.preventDefault();
                e.stopPropagation();
                if (worldPreviewAvailable) {
                    toggleWorldOverlay();
                    oneWorldOverlayHidden = !worldOverlayEnabled;
                }
            });

            editToolbar.appendChild(toggleBtn);
            refreshToolLabel(toggleBtn);

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

        if (worldOverlayEnabled) {
            // Show overlay
            var data = worldOverlayData;

            if (worldOverlay) {
                map.removeLayer(worldOverlay);
            }

            worldOverlay = buildWorldOverlay(data, worldOpacity());
            worldOverlay.addTo(map);

            if (btn) {
                btn.classList.add('active');
                refreshToolLabel(btn);
            }
        } else {
            // Hide overlay
            if (worldOverlay) {
                map.removeLayer(worldOverlay);
                worldOverlay = null;
            }
            if (btn) {
                btn.classList.remove('active');
                refreshToolLabel(btn);
            }
        }
        updateSliderVisibility();
    }

    // Enable the preview button when data is available
    function enableWorldPreview(data) {
        // Skip world preview when rotation is active — the preview image covers
        // the expanded post-rotation MC bbox but the geo bounds are pre-rotation,
        // so the image would be squeezed incorrectly onto the map.
        if (Math.abs(window._rotationAngle || 0) >= 0.001) {
            return;
        }
        worldOverlayData = data;
        worldPreviewAvailable = true;
        var slider = document.getElementById('world-preview-opacity');
        if (slider) slider.value = String((balanceOn() ? overlayBalance : overlayOpacity)[sliderKind()]);
        var btn = document.getElementById('world-preview-btn');
        if (btn) {
            btn.classList.remove('disabled');
            refreshToolLabel(btn);
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
        if (btn) {
            btn.classList.add('disabled');
            btn.classList.remove('active');
            refreshToolLabel(btn);
        }
        updateSliderVisibility();
    }



    // ========== Context Menu for Coordinate Copying ==========
    var contextMenuElement = null;

    // Create the context menu element
    function createContextMenu() {
        if (contextMenuElement) return contextMenuElement;

        contextMenuElement = document.createElement('div');
        contextMenuElement.className = 'coordinate-context-menu';
        contextMenuElement.style.display = 'none';
        contextMenuElement.innerHTML = `
            <div class="coordinate-context-menu-item" id="copy-coords-item">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                </svg>
                <span id="copy-coords-text">Copy coordinates</span>
            </div>
            <div class="coordinate-context-menu-item" id="live-teleport-item">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M20 10c0 6-8 12-8 12s-8-6-8-12a8 8 0 0 1 16 0Z"></path>
                    <circle cx="12" cy="10" r="3"></circle>
                </svg>
                <span id="live-teleport-text">Teleport here</span>
            </div>
            <div class="coordinate-context-menu-item" id="remove-area-item">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M3 6h18"></path>
                    <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"></path>
                    <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"></path>
                </svg>
                <span id="remove-area-text">Remove this area</span>
            </div>
        `;
        document.body.appendChild(contextMenuElement);

        contextMenuElement.querySelector('#remove-area-item').addEventListener('click', function(e) {
            e.preventDefault();
            e.stopPropagation();
            var target = contextMenuElement._area;
            hideContextMenu();
            if (!target || !drawnItems.hasLayer(target)) return;
            // The delete path resets bounds, notifies the parent and refreshes handles.
            map.fire('draw:deleted', { layers: L.layerGroup([target]) });
        });

        contextMenuElement.querySelector('#live-teleport-item').addEventListener('click', function(e) {
            e.preventDefault();
            e.stopPropagation();
            window.parent.postMessage({
                type: 'liveTeleport',
                lat: parseFloat(contextMenuElement.dataset.lat),
                lng: parseFloat(contextMenuElement.dataset.lng)
            }, '*');
            hideContextMenu();
        });

        // Handle click on the copy coordinates item
        var copyItem = contextMenuElement.querySelector('#copy-coords-item');
        copyItem.addEventListener('click', function(e) {
            e.preventDefault();
            e.stopPropagation();
            copyMinecraftCoordinates();
            hideContextMenu();
        });

        return contextMenuElement;
    }

    // Show context menu at position
    function showContextMenu(x, y, latLng, canCopy, area, cells) {
        var menu = createContextMenu();
        fillCellActions(menu, cells || []);
        menu._area = area || null;
        menu.querySelector('#copy-coords-item').style.display = canCopy ? '' : 'none';
        menu.querySelector('#live-teleport-item').style.display = liveCanTeleport ? '' : 'none';
        menu.querySelector('#remove-area-item').style.display = area ? '' : 'none';
        menu.querySelector('#copy-coords-text').textContent = mapText('map_copy_coordinates', 'Copy coordinates');
        menu.querySelector('#live-teleport-text').textContent = mapText('live_teleport', 'Teleport here');
        menu.querySelector('#remove-area-text').textContent = mapText('map_remove_area', 'Remove this area');

        // Position the menu, ensuring it stays within viewport
        menu.style.display = 'block';
        var menuWidth = menu.offsetWidth || 180;
        var menuHeight = menu.offsetHeight || 4 + 36 * [canCopy, liveCanTeleport, !!area].filter(Boolean).length;
        var viewportWidth = window.innerWidth;
        var viewportHeight = window.innerHeight;

        var posX = x;
        var posY = y;

        // Adjust if menu would go off-screen
        if (x + menuWidth > viewportWidth) {
            posX = viewportWidth - menuWidth - 10;
        }
        if (y + menuHeight > viewportHeight) {
            posY = viewportHeight - menuHeight - 10;
        }

        menu.style.left = posX + 'px';
        menu.style.top = posY + 'px';
        menu.style.display = 'block';

        // Store the latLng for copying
        menu.dataset.lat = latLng.lat;
        menu.dataset.lng = latLng.lng;
        // The keyboard starts on the first cell action.
        var first = menu.querySelector('.cell-actions [role="menuitem"]');
        if (first) first.focus();
    }

    // The cell actions at the top of the menu: the count, then what fits every picked cell. An action
    // the head cell cannot take says why, instead of going grey without a word.
    function fillCellActions(menu, cells) {
        var old = menu.querySelector('.cell-actions');
        if (old) old.remove();
        if (!cells.length) return;
        var records = lastPieces.filter(function(p) { return cells.indexOf(p.i) >= 0; });
        var box = document.createElement('div');
        box.className = 'cell-actions';
        var head = document.createElement('div');
        head.className = 'cell-actions-head';
        head.textContent = cellCount(cells.length);
        box.appendChild(head);
        var actions = window.arnisCells.actions(records, cellsRunning);
        actions.forEach(function(a) {
            var item = document.createElement('div');
            item.className = 'coordinate-context-menu-item' + (a.why ? ' is-off' : '');
            item.setAttribute('role', 'menuitem');
            // Reached with Tab and taken with Enter or Space, as a button.
            item.tabIndex = 0;
            item.addEventListener('keydown', function(e) {
                if (e.key !== 'Enter' && e.key !== ' ') return;
                e.preventDefault();
                item.click();
            });
            var label = document.createElement('span');
            label.textContent = mapText(a.label[0], a.label[1]);
            item.appendChild(label);
            if (a.why) {
                var why = document.createElement('small');
                why.textContent = mapText(a.why[0], a.why[1]);
                item.appendChild(why);
                item.setAttribute('aria-disabled', 'true');
            } else {
                item.addEventListener('click', function(e) {
                    e.preventDefault();
                    e.stopPropagation();
                    hideContextMenu();
                    window.parent.postMessage({ type: 'cellsAction', action: a.id, cells: cells }, '*');
                });
            }
            box.appendChild(item);
        });
        if (!actions.length) {
            var none = document.createElement('div');
            none.className = 'cell-actions-none';
            none.textContent = mapText('cell_none', 'No action fits all of these cells.');
            box.appendChild(none);
        }
        var sep = document.createElement('div');
        sep.className = 'coordinate-context-menu-separator';
        box.appendChild(sep);
        menu.insertBefore(box, menu.firstChild);
    }

    // The count of cells in the language shown ("12 cells"); the window writes it.
    function cellCount(n) {
        var f = window.parent && window.parent.arnisCellCount;
        return f ? f(n) : n + ' cells';
    }

    // Hide context menu
    function hideContextMenu() {
        if (contextMenuElement) {
            contextMenuElement.style.display = 'none';
        }
    }

    // Calculate Minecraft coordinates from lat/lng
    function calculateMinecraftCoords(lat, lng) {
        if (!worldOverlayData) return null;

        var data = worldOverlayData;

        // One World: same projection as src/projection/web_mercator.rs.
        if (data.areas) {
            var R = 6371000.0;
            var rad = Math.PI / 180;
            var k = data.scale * Math.cos(data.origin_lat * rad);
            var mercY = function (latDeg) {
                return R * Math.log(Math.tan(Math.PI / 4 + latDeg * rad / 2));
            };
            var x = Math.floor(R * (lng - data.origin_lon) * rad * k);
            var z = Math.floor(-(mercY(lat) - mercY(data.origin_lat)) * k);
            // Terrain can be anywhere from Y -2014 to 2016, so land on the top block.
            return { x: x, y: 100, z: z, command: '/spreadplayers ' + x + ' ' + z + ' 0 1 false @s' };
        }

        // Check if Minecraft coordinate bounds are available (not all zeros)
        if (data.min_mc_x === 0 && data.max_mc_x === 0 && 
            data.min_mc_z === 0 && data.max_mc_z === 0) {
            return null;
        }

        // Calculate the relative position within the geo bounds (0 to 1)
        // Note: Latitude increases northward, but Minecraft Z increases southward
        var relX = (lng - data.min_lon) / (data.max_lon - data.min_lon);
        var relZ = (data.max_lat - lat) / (data.max_lat - data.min_lat);

        // Clamp to 0-1 range
        relX = Math.max(0, Math.min(1, relX));
        relZ = Math.max(0, Math.min(1, relZ));

        // Calculate Minecraft X and Z coordinates
        var mcX = Math.round(data.min_mc_x + relX * (data.max_mc_x - data.min_mc_x));
        var mcZ = Math.round(data.min_mc_z + relZ * (data.max_mc_z - data.min_mc_z));

        // Default Y coordinate (ground level, typically around 64-70)
        var mcY = 100;

        return { x: mcX, y: mcY, z: mcZ };
    }

    // Copy Minecraft coordinates to clipboard
    function copyMinecraftCoordinates() {
        if (!contextMenuElement) return;

        var lat = parseFloat(contextMenuElement.dataset.lat);
        var lng = parseFloat(contextMenuElement.dataset.lng);

        var coords = calculateMinecraftCoords(lat, lng);
        if (!coords) return;

        var tpCommand = coords.command || ('/tp ' + coords.x + ' ' + coords.y + ' ' + coords.z);

        // Copy to clipboard using modern API with fallback
        if (navigator.clipboard && navigator.clipboard.writeText) {
            navigator.clipboard.writeText(tpCommand).catch(function(err) {
                // Fallback for clipboard API failure
                fallbackCopyToClipboard(tpCommand);
            });
        } else {
            // Fallback for older browsers
            fallbackCopyToClipboard(tpCommand);
        }
    }

    // Fallback clipboard copy method for older browsers
    function fallbackCopyToClipboard(text) {
        var textArea = document.createElement('textarea');
        textArea.value = text;
        textArea.style.position = 'fixed';
        textArea.style.left = '-9999px';
        textArea.style.top = '-9999px';
        document.body.appendChild(textArea);
        textArea.focus();
        textArea.select();

        try {
            document.execCommand('copy');
        } catch (err) {
            console.error('Failed to copy coordinates:', err);
        }

        document.body.removeChild(textArea);
    }

    // Check if Minecraft coordinate bounds are available
    function hasMinecraftCoords() {
        if (!worldOverlayData) return false;
        var data = worldOverlayData;
        return !(data.min_mc_x === 0 && data.max_mc_x === 0 && 
                 data.min_mc_z === 0 && data.max_mc_z === 0);
    }

    // Handle right-click on the map
    map.on('contextmenu', function(e) {
        // Coordinates are offered inside the world; a live session can teleport anywhere.
        var canCopy = false;
        if (worldPreviewAvailable && worldOverlayData && hasMinecraftCoords()) {
            var data = worldOverlayData;
            var lat = e.latlng.lat;
            var lng = e.latlng.lng;

            var inside = function (b) {
                return lat >= b.min_lat && lat <= b.max_lat && lng >= b.min_lon && lng <= b.max_lon;
            };
            canCopy = data.areas ? data.areas.some(inside) : inside(data);
        }
        // During a live session the copied command would jump onto land not built yet.
        if (liveActive) canCopy = false;
        var area = areaAt(e.latlng);
        // With the select tool on, a cell under the pointer: the menu acts on the picked cells (the cell
        // alone if it was not one of them).
        var cells = [];
        var hit = cellTool ? window.arnisCells.at(lastPieces, e.latlng.lat, e.latlng.lng) : -1;
        if (hit >= 0) {
            if (!cellPick.has(hit)) pickCells(new Set([hit]));
            cells = Array.from(cellPick).sort(function(a, b) { return a - b; });
        }
        if (canCopy || liveCanTeleport || area || cells.length) {
            showContextMenu(e.originalEvent.clientX, e.originalEvent.clientY, e.latlng, canCopy, area, cells);
        }
    });

    // Hide context menu on any click or map interaction
    document.addEventListener('click', function(e) {
        if (contextMenuElement && !contextMenuElement.contains(e.target)) {
            hideContextMenu();
        }
    });

    map.on('movestart', hideContextMenu);
    map.on('zoomstart', hideContextMenu);
    // ========== End Context Menu ==========

    // Live mode: a marker per player in the overworld, and teleporting from the menu.
    var liveActive = false;
    // Teleports only work once the server runs.
    var liveCanTeleport = false;
    var livePlayerLayer = L.layerGroup().addTo(map);
    var livePlayerMarkers = {};

    function setLivePlayers(active, canTeleport, players) {
        liveActive = !!active;
        if (liveCanTeleport && !canTeleport) hideContextMenu();
        liveCanTeleport = liveActive && !!canTeleport;
        var seen = {};
        (liveActive ? players || [] : []).forEach(function (p) {
            if (typeof p.lat !== 'number' || typeof p.lon !== 'number') return;
            seen[p.name] = true;
            var at = [p.lat, p.lon];
            var marker = livePlayerMarkers[p.name];
            if (marker) {
                marker.setLatLng(at);
            } else {
                var icon = L.divIcon({
                    className: 'live-player-marker',
                    html: '<span class="live-player-dot"></span><span class="live-player-name"></span>',
                    iconSize: [18, 18],
                    iconAnchor: [9, 9]
                });
                marker = L.marker(at, { icon: icon, interactive: false, keyboard: false, zIndexOffset: 3000 });
                marker.addTo(livePlayerLayer);
                var label = marker.getElement() && marker.getElement().querySelector('.live-player-name');
                if (label) label.textContent = p.name;
                livePlayerMarkers[p.name] = marker;
            }
            // The face replaces the dot once Mojang sent the skin.
            var el = marker.getElement();
            if (p.head && el && marker._arnisHead !== p.head && /^data:image\/png;base64,/.test(p.head)) {
                marker._arnisHead = p.head;
                var dot = el.querySelector('.live-player-dot');
                if (dot) {
                    var img = document.createElement('img');
                    img.className = 'live-player-head';
                    img.alt = '';
                    img.src = p.head;
                    dot.replaceWith(img);
                } else {
                    var head = el.querySelector('.live-player-head');
                    if (head) head.src = p.head;
                }
            }
        });
        Object.keys(livePlayerMarkers).forEach(function (name) {
            if (!seen[name]) {
                livePlayerLayer.removeLayer(livePlayerMarkers[name]);
                delete livePlayerMarkers[name];
            }
        });
    }

    // Coordinates typed into the parent's bbox field, as [south, west, north,
    // east]. Applied through the same draw:created path the hash restore uses,
    // so bounds, handles, the location hash and the parent notification all
    // stay in exactly one place.
    function applyBboxFromParent(b) {
        var valid = Object.prototype.toString.call(b) === '[object Array]' &&
            b.length === 4 &&
            b.every(function (n) { return typeof n === 'number' && isFinite(n); });
        if (!valid) {
            arnisLog('warn', 'Ignoring malformed bbox from the parent window');
            return;
        }

        // A spawn point picked for the previous area is meaningless here, and
        // the frame reload this replaces used to drop it too.
        if (drawnItems) {
            drawnItems.eachLayer(function (layer) {
                if (layer instanceof L.Marker) drawnItems.removeLayer(layer);
            });
        }

        var lyr = new L.Rectangle(new L.LatLngBounds([b[0], b[1]], [b[2], b[3]]), {
            color: '#3778d4',
            opacity: 1.0,
            weight: 3,
            fill: '#3778d4',
            lineCap: 'round',
            lineJoin: 'round'
        });
        // Restored rectangles fire as "polygon"; the handler keys off instanceof.
        map.fire('draw:created', { layer: lyr, layerType: 'polygon' });
    }
    // A place picked in the search (search.js) becomes the selection the same way.
    window.arnisSelectBounds = applyBboxFromParent;

    // An outline (GeoJSON Polygon or MultiPolygon, from the shape tool or a search result):
    // its bounding box becomes the selection and the outline the plan's mask.
    window.arnisSelectShape = function (geometry) {
        var b = L.geoJSON(geometry).getBounds();
        applyBboxFromParent([b.getSouth(), b.getWest(), b.getNorth(), b.getEast()]);
        setPlanMask(geometry);
    };

    var pieceLayer = L.layerGroup().addTo(map);
    var stretchLayer = L.layerGroup().addTo(map);
    // The colours and line patterns live in piece-style.js, which the legend reads too.
    var PIECE_COLORS = window.arnisPieceStyle.colors;
    // How full each state's cup is: a running piece fills to its percent, a finished one to the top.
    var PIECE_FULL = { done: 1, skipped: 1, failed: 1, stopped: 1 };
    // A cell's colour and line pattern say its state; its label is the number alone.
    var PIECE_DASH = window.arnisPieceStyle.dash;
    var lastPieces = [];
    var pieceHalo = false;
    // Below this width on screen (px) a piece shows no number: zoomed out, thousands of numbers
    // would only clutter the map.
    var PIECE_LABEL_MIN_PX = 64;

    function pieceElapsed(p) {
        if (!p.since) return '';
        var s = Math.max(0, Math.round(((p.end || Date.now()) - p.since) / 1000));
        return Math.floor(s / 60) + ':' + ('0' + (s % 60)).slice(-2);
    }

    function pieceTip(p) {
        var row = function(text) {
            var d = document.createElement('div');
            d.textContent = text;
            return d;
        };
        var box = document.createElement('div');
        var running = p.state === 'start' || p.state === 'retry';
        // Line 1: the state's dot and word (and percent), the piece's name muted after it.
        var head = row(p.text.state + (running && p.pct != null ? ' ' + p.pct + '%' : ''));
        head.className = 'piece-tip-head';
        var dot = document.createElement('i');
        dot.className = 'piece-tip-dot';
        dot.style.background = PIECE_COLORS[p.state] || '#8a8a8a';
        var name = document.createElement('span');
        name.textContent = p.text.piece;
        head.prepend(dot);
        head.appendChild(name);
        box.appendChild(head);
        if (p.text.size) box.appendChild(row(p.text.size));
        if (p.text.worker) box.appendChild(row(p.text.worker));
        if (p.since) box.appendChild(row(pieceElapsed(p)));
        if (p.text.error) box.appendChild(row(p.text.error));
        if (p.text.hint) box.appendChild(row(p.text.hint));
        if (p.text.openLog) {
            var open = row(p.text.openLog);
            open.className = 'piece-tip-log';
            open.onclick = function(e) {
                e.stopPropagation();
                window.parent.postMessage({ type: 'openLog', piece: p.i }, '*');
            };
            box.appendChild(open);
        }
        return box;
    }

    // One outline, label and tooltip per piece, kept between records and updated in place: a
    // progress record every second must not rebuild the map layers or close an open tooltip.
    var pieceLayers = {};
    var heldCard = null;

    // At most this many cells are drawn at once: the ones inside the view. Zoomed out past that,
    // only the job's outline is drawn; zooming in brings the cells back (moveend below).
    var CELL_CAP = 2000;
    var cellExtent = null;
    var cellShade = null;
    var cellAreaKey = '';

    // The cell select tool: on, the cells picked (plan indexes), a drag in progress, and the button.
    var cellTool = false;
    var cellPick = new Set();
    var cellsRunning = false;
    var cellDrag = null;
    var cellBoxLayer = null;
    var cellBtn = null;

    // A hatched fill for the cells left out of the job; the SVG pattern goes in the cell pane's svg once.
    function ensureHatch(layer) {
        var svg = layer._renderer && layer._renderer._container;
        if (!svg || svg.querySelector('#arnis-hatch')) return;
        var ns = 'http://www.w3.org/2000/svg';
        var pat = document.createElementNS(ns, 'pattern');
        pat.setAttribute('id', 'arnis-hatch');
        pat.setAttribute('patternUnits', 'userSpaceOnUse');
        pat.setAttribute('width', '6');
        pat.setAttribute('height', '6');
        pat.setAttribute('patternTransform', 'rotate(45)');
        var line = document.createElementNS(ns, 'line');
        line.setAttribute('y2', '6');
        line.setAttribute('stroke', PIECE_COLORS.omitted);
        line.setAttribute('stroke-width', '2');
        pat.appendChild(line);
        var defs = document.createElementNS(ns, 'defs');
        defs.appendChild(pat);
        svg.insertBefore(defs, svg.firstChild);
    }

    function drawPieceOutlines(pieces) {
        lastPieces = pieces;
        syncCellTool();
        // While pieces build, the selection and its handles fade so the pieces read (bbox.css).
        document.body.classList.toggle('pieces-building', pieces.some(function(p) { return p.state === 'start' || p.state === 'retry'; }));
        var view = map.getBounds();
        var inView = window.arnisCellsInView(pieces, [view.getSouth(), view.getWest(), view.getNorth(), view.getEast()], CELL_CAP);
        var extent = inView.over ? window.arnisCellsExtent(pieces) : null;
        // One faint dark fill under the whole plan area, so the cells' colours and numbers read
        // against the map. Below the outlines, gone with the grid.
        var area = window.arnisCellsExtent(pieces);
        // Both are drawn again only when they move.
        var areaKey = String(extent) + '/' + String(area);
        if (areaKey !== cellAreaKey) {
            cellAreaKey = areaKey;
            if (cellExtent) { pieceLayer.removeLayer(cellExtent); cellExtent = null; }
            if (extent) {
                cellExtent = L.rectangle([[extent[0], extent[1]], [extent[2], extent[3]]], {
                    color: PIECE_COLORS.planned, weight: 2, fill: false, interactive: false, pane: 'arnisCells'
                }).addTo(pieceLayer);
            }
            if (cellShade) { pieceLayer.removeLayer(cellShade); cellShade = null; }
            if (area) {
                cellShade = L.rectangle([[area[0], area[1]], [area[2], area[3]]], {
                    stroke: false, fillColor: '#000', fillOpacity: 0.15, interactive: false, pane: 'arnisCells'
                }).addTo(pieceLayer);
                cellShade.bringToBack();
            }
        }
        // Past the cap only the job's outline is drawn: a first 2,000 cells would be a patch, not a grid.
        pieces = inView.over ? [] : inView.cells;
        var seen = {};
        var moved = false;
        pieces.forEach(function(p) {
            seen[p.i] = true;
            var color = PIECE_COLORS[p.state] || '#8a8a8a';
            var running = p.state === 'start' || p.state === 'retry';
            var retryable = Boolean(p.text.hint);
            var b = p.bounds;
            var item = pieceLayers[p.i];
            if (!item) {
                moved = true;
                item = pieceLayers[p.i] = {};
                item.rect = L.rectangle([[b[0], b[1]], [b[2], b[3]]], { interactive: true, pane: 'arnisCells' }).addTo(pieceLayer);
                ensureHatch(item.rect);
                // A failed piece's card holds an "Open log" button, so it must be reachable: it
                // stays put (not sticky) and closes a moment after the pointer leaves both.
                // Leaflet registers closeTooltip when binding, so the wrapper goes on first.
                var closeNow = item.rect.closeTooltip;
                item.holds = function() {
                    var tip = item.rect.getTooltip() && item.rect.getTooltip().getElement();
                    return window.arnisCardHold(Boolean(tip && tip.matches(':hover')), item.leftAt, Date.now());
                };
                item.rect.closeTooltip = function() {
                    if (!item.p.text.openLog) return closeNow.call(item.rect);
                    clearTimeout(item.closing);
                    (function poll() {
                        if (item.holds()) { item.closing = setTimeout(poll, 150); return; }
                        if (heldCard === item) heldCard = null;
                        closeNow.call(item.rect);
                    })();
                    return item.rect;
                };
                item.rect.on('mouseover', function() { item.leftAt = null; });
                item.rect.on('mouseout', function() { item.leftAt = Date.now(); });
                item.rect.bindTooltip(pieceTip(p), { sticky: true, interactive: true, className: 'piece-tip' });
                item.rect.on('tooltipopen', function(e) {
                    // While the pointer travels to a failed piece's card, a neighbour's card
                    // does not take its place.
                    if (heldCard && heldCard !== item && heldCard.holds()) return closeNow.call(item.rect);
                    // The card is made as it opens: its clock is not redrawn on every record.
                    item.rect.setTooltipContent(pieceTip(item.p));
                    if (item.p.text.openLog) heldCard = item;
                    e.tooltip.options.sticky = !item.p.text.openLog;
                    e.tooltip.getElement().onmouseleave = function() { item.leftAt = 0; item.rect.closeTooltip(); };
                });
                item.rect.on('click', function() {
                    if (!cellTool && item.p.text.hint) window.parent.postMessage({ type: 'retryPiece', piece: item.p.i }, '*');
                });
                item.label = L.tooltip({ permanent: true, direction: 'center', className: 'piece-label', interactive: false, pane: 'arnisCellLabels' })
                    .setLatLng(item.rect.getBounds().getCenter()).addTo(pieceLayer);
            }
            // A plan drawn again may give this number another place: a layer kept by number but
            // left where it was drew an old plan's piece over the new one's.
            if (!item.rect.getBounds().equals([[b[0], b[1]], [b[2], b[3]]])) {
                item.rect.setBounds(L.latLngBounds([b[0], b[1]], [b[2], b[3]]));
                item.label.setLatLng(item.rect.getBounds().getCenter());
                item.sig = null;
                moved = true;
            }
            item.p = p;
            // A cell drawn as it is now is left alone: a large job sends many records a second.
            var picked = cellPick.has(p.i);
            var sig = [p.state, p.pct, p.n, picked, p.ring, pieceHalo && p.halo ? p.halo.join() : '', p.since, p.end,
                p.text.state, p.text.piece, p.text.size, p.text.worker, p.text.error, p.text.hint, p.text.openLog].join('|');
            if (sig === item.sig) return;
            item.sig = sig;
            // A plan's pieces are thin outlines, its ring (margin) fainter and dotted. A job's
            // pieces are empty cups that fill from the bottom.
            // Outside the outline: a plan's piece, greyed, that is not built.
            // Room to grow (within 5% stretch): grey, dotted, empty.
            var excluded = p.state === 'excluded';
            var room = p.state === 'room';
            var planned = p.state === 'planned' || excluded || room;
            // A picked cell: the pick colour, a thick line and a fill. A skipped one is hatched.
            item.rect.setStyle({
                color: picked ? PIECE_COLORS.picked : color,
                weight: picked ? 4 : room ? 1.5 : planned ? 1 : p.state === 'failed' ? 3 : 2,
                fillColor: picked ? PIECE_COLORS.picked : p.state === 'omitted' ? 'url(#arnis-hatch)' : color,
                opacity: room ? 0.9 : 1,
                fillOpacity: picked ? 0.3 : p.state === 'omitted' ? 0.9 : excluded ? 0.3 : room ? 0 : planned ? (p.ring ? 0.03 : 0.08) : 0,
                dashArray: picked ? null : room ? PIECE_DASH.room : planned && p.ring ? PIECE_DASH.ring : PIECE_DASH[p.state] || null
            });
            var full = running ? (p.pct || 0) / 100 : PIECE_FULL[p.state] || 0;
            if (full > 0) {
                var cup = [[b[0], b[1]], [b[0] + (b[2] - b[0]) * Math.min(1, full), b[3]]];
                if (!item.fill) item.fill = L.rectangle(cup, { stroke: false, interactive: false, fillOpacity: 0.35, pane: 'arnisCells' }).addTo(pieceLayer);
                else item.fill.setBounds(cup);
                item.fill.setStyle({ fillColor: color });
            } else if (item.fill) {
                pieceLayer.removeLayer(item.fill);
                item.fill = null;
            }
            var path = item.rect.getElement();
            if (path) {
                path.classList.toggle('piece-running', running);
                path.classList.toggle('piece-retryable', retryable);
            }
            item.rect.setTooltipContent(pieceTip(p));
            item.label.setContent(p.n + (running && p.pct != null ? ' ' + p.pct + '%' : ''));
            if (item.halo) { pieceLayer.removeLayer(item.halo); item.halo = null; }
            if (pieceHalo && p.halo) {
                item.halo = L.rectangle([[p.halo[0], p.halo[1]], [p.halo[2], p.halo[3]]], {
                    color: color, weight: 1, opacity: 0.4, fill: false, dashArray: PIECE_DASH.halo, interactive: false, pane: 'arnisCells'
                }).addTo(pieceLayer);
            }
        });
        Object.keys(pieceLayers).forEach(function(i) {
            if (seen[i]) return;
            var item = pieceLayers[i];
            [item.rect, item.label, item.halo, item.fill].forEach(function(l) { if (l) pieceLayer.removeLayer(l); });
            heldCard = window.arnisCardDrop(item, heldCard);
            delete pieceLayers[i];
        });
        if (moved) updatePieceLabels();
        applyBalance();
        updateSliderVisibility();
    }

    // A label stays out of a piece too small on screen to hold it (its shorter side counts).
    function updatePieceLabels() {
        Object.keys(pieceLayers).forEach(function(i) {
            var item = pieceLayers[i];
            var sw = map.latLngToContainerPoint(item.rect.getBounds().getSouthWest());
            var ne = map.latLngToContainerPoint(item.rect.getBounds().getNorthEast());
            var side = Math.min(Math.abs(ne.x - sw.x), Math.abs(ne.y - sw.y));
            var el = item.label.getElement();
            if (!el) return;
            el.classList.toggle('piece-label-off', side < PIECE_LABEL_MIN_PX || item.p.n === '');
            // The number grows with the piece on screen.
            var size = Math.max(14, Math.min(30, Math.round(side / 4))) + 'px';
            // Re-centred only when the size changes: Leaflet centres by the label's width.
            if (el.style.fontSize !== size) {
                el.style.fontSize = size;
                item.label.update();
            }
        });
    }
    map.on('zoomend', updatePieceLabels);
    map.on('moveend', function() { if (lastPieces.length) drawPieceOutlines(lastPieces); });

    // Listen for messages from parent window
    window.addEventListener('message', function(event) {
        if (event.source !== window.parent) return;
        if (event.data && event.data.type === 'changeTileTheme') {
            changeTileTheme(event.data.theme);
        }

        // User-supplied tile template from the settings panel
        if (event.data && event.data.type === 'setCustomTileUrl') {
            setCustomTileUrl(event.data.url);
        }

        // Coordinates typed into the parent's bbox field
        if (event.data && event.data.type === 'setBbox') {
            applyBboxFromParent(event.data.bounds);
        }

        // Earth / Moon / Mars picked in the settings modal
        if (event.data && event.data.type === 'changeBody') {
            changeBody(event.data.body);
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

        // A job built in pieces: one outline per piece, coloured by its state.
        // The window could not use the outline: take it off the map.
        if (event.data && event.data.type === 'planMask' && !event.data.geojson) {
            setPlanMask(null);
        }

        if (event.data && event.data.type === 'pieceOutlines') {
            cellsRunning = Boolean(event.data.running);
            drawPieceOutlines(event.data.pieces || []);
        }

        // Esc in the window clears the picked cells too.
        if (event.data && event.data.type === 'cellsClear') {
            pickCells(new Set());
        }

        // A One World's stretch limits: dashed lines across the map at 5% and 10%.
        if (event.data && event.data.type === 'stretchLines') {
            stretchLayer.clearLayers();
            (event.data.lines || []).forEach(function(l) {
                L.polyline([[l.lat, -180], [l.lat, 180]], { color: l.color, weight: 2, opacity: 0.9, dashArray: PIECE_DASH.stretch, interactive: false }).addTo(stretchLayer);
                L.tooltip({ permanent: true, direction: 'top', className: 'piece-label', interactive: false })
                    .setLatLng([l.lat, l.lon]).setContent(l.label).addTo(stretchLayer);
            });
        }

        // The faint border around each piece, off until the user asks for it.
        if (event.data && event.data.type === 'pieceHalo') {
            pieceHalo = Boolean(event.data.on);
            drawPieceOutlines(lastPieces);
        }

        // Handle world changed (disable preview)
        if (event.data && event.data.type === 'worldChanged') {
            disableWorldPreview();
        }

        // One World: every generated area. An empty list clears them.
        if (event.data && event.data.type === 'oneWorldOverlays') {
            var areas = event.data.areas || [];
            var worldPath = event.data.world_path || null;
            if (worldPath !== oneWorldFittedPath) oneWorldOverlayHidden = false;
            disableWorldPreview();
            if (areas.length > 0) {
                enableWorldPreview({
                    areas: areas,
                    origin_lat: event.data.origin_lat,
                    origin_lon: event.data.origin_lon,
                    scale: event.data.scale
                });
                if (worldPreviewAvailable && !oneWorldOverlayHidden) {
                    toggleWorldOverlay();
                }
                if (globalThis.arnisShouldFit(worldPath, oneWorldFittedPath, event.data.refit)) {
                    var bounds = oneWorldBounds(areas);
                    if (bounds) map.fitBounds(bounds, { padding: [50, 50] });
                }
            }
            oneWorldFittedPath = areas.length > 0 ? worldPath : null;
        }

        if (event.data && event.data.type === 'livePlayers') {
            setLivePlayers(event.data.active, event.data.canTeleport, event.data.players);
        }

        // Advanced features on: a country or region found by the search can be selected whole.
        if (event.data && event.data.type === 'meldFeatures') {
            var was = window.arnisMeldOn;
            window.arnisMeldOn = Boolean(event.data.on);
            window.arnisWorldScale = Number(event.data.scale) || 1;
            window.arnisSnapOn = Boolean(event.data.snap);
            // The map-first page's look (hover font, previews under the selection); Classic keeps its own.
            window.arnisMapFirst = Boolean(event.data.mapFirst);
            document.body.classList.toggle('map-first', window.arnisMapFirst);
            if (!window.arnisMeldOn && planMask) setPlanMask(null);
            if (was !== window.arnisMeldOn) {
                refreshBboxHandles();
                syncSliderMode();
            }
        }

        // One World keeps rotation at 0.
        if (event.data && event.data.type === 'setRotationLocked') {
            setRotationLocked(!!event.data.locked);
        }
        // One World takes rectangles only ('locked'); with Advanced features it builds the
        // rectangle around the shapes ('rect'), and in pieces keeps them ('pieces').
        if (event.data && event.data.type === 'setShapesMode') {
            _shapesMode = event.data.mode || '';
            setShapesLocked(_shapesMode === 'locked');
            refreshShapesRect();
        }

        // Handle rotation preview angle update (store it for preview-skip logic)
        if (event.data && event.data.type === 'rotatePreview') {
            var angle = event.data.angle || 0;
            window._rotationAngle = angle;
            // Clear the world preview since it won't align at a different angle
            if (worldOverlayEnabled && Math.abs(angle) >= 0.001) {
                disableWorldPreview();
            }
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
    crosshair = new L.marker(map.getCenter(), { icon: crosshairIcon, interactive: false });
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

    // ========== Area shapes ==========
    // The selection: one or more rectangles and drawn shapes (L.Rectangle is an L.Polygon too).
    var AREA_STYLE = {
        color: '#fecc44',
        opacity: 1.0,
        weight: 3,
        fill: '#fecc44',
        fillOpacity: 0.08,
        lineCap: 'round',
        lineJoin: 'round'
    };

    function isArea(layer) {
        return layer instanceof L.Polygon;
    }

    function areaLayers() {
        var out = [];
        drawnItems.eachLayer(function (l) { if (isArea(l)) out.push(l); });
        return out;
    }

    // The selection when it is one rectangle, which the Advanced handles work on; else null.
    function _getBboxRect() {
        var areas = areaLayers();
        return areas.length === 1 && areas[0] instanceof L.Rectangle ? areas[0] : null;
    }

    function ringOf(layer) {
        var ll = layer.getLatLngs();
        return L.LineUtil.isFlat(ll) ? ll : ll[0];
    }

    // [lat, lng] points on the map copy around longitude 0; null across the 180° meridian.
    function normalizedRing(layer) {
        var ring = ringOf(layer);
        var box = L.latLngBounds(ring);
        var west = box.getWest(), east = box.getEast();
        var shift = -360 * Math.round((west + east) / 720);
        if (west + shift < -180 || east + shift > 180) return null;
        return ring.map(function (p) { return [p.lat, p.lng + shift]; });
    }

    // Whether these areas, moved onto the copy around longitude 0, span at most 180° together.
    function fitOneSide(areas) {
        var all = L.latLngBounds([]);
        for (var i = 0; i < areas.length; i++) {
            var ring = normalizedRing(areas[i]);
            if (!ring) return false;
            all.extend(L.latLngBounds(ring));
        }
        return !all.isValid() || all.getEast() - all.getWest() <= 180;
    }

    // Earth's "extensive" size tier at scale 1, in m²: past it, a mostly empty rectangle is refused.
    var FAR_APART_M2 = 100e6;

    // Whether the rectangle around these areas, all of which is downloaded and processed, is
    // large and mostly empty: such areas are better generated as worlds of their own.
    function tooFarApart(areas) {
        if (areas.length < 2 || currentBody !== 'earth') return false;
        var rad = Math.PI / 180, R = 6371000;
        var size = function (b) {
            return R * R * (b.getNorth() - b.getSouth()) * rad * (b.getEast() - b.getWest()) * rad * Math.cos((b.getSouth() + b.getNorth()) / 2 * rad);
        };
        var own = 0, all = L.latLngBounds([]);
        for (var i = 0; i < areas.length; i++) {
            var ring = normalizedRing(areas[i]);
            if (!ring) return false;
            var b = L.latLngBounds(ring);
            own += size(b);
            all.extend(b);
        }
        var total = size(all);
        return total > FAR_APART_M2 && total > 2 * own;
    }

    // For generation: null for a single rectangle (the plain bbox), else every area as a ring.
    function selectionShapes() {
        var areas = areaLayers();
        if (areas.length === 0 || (areas.length === 1 && areas[0] instanceof L.Rectangle)) return null;
        // Sent as is across the meridian, so the backend refuses it instead of silently dropping it.
        return areas.map(function (l) {
            return normalizedRing(l) || ringOf(l).map(function (p) { return [p.lat, p.lng]; });
        });
    }
    window.arnisSelectionShapes = selectionShapes;

    // Point in an area: by bounds for a rectangle, even-odd for a shape.
    function areaContains(layer, latlng) {
        if (layer instanceof L.Rectangle) return layer.getBounds().contains(latlng);
        var ring = ringOf(layer), inside = false;
        for (var i = 0, j = ring.length - 1; i < ring.length; j = i++) {
            var a = ring[i], b = ring[j];
            if ((a.lat > latlng.lat) !== (b.lat > latlng.lat) &&
                latlng.lng < (b.lng - a.lng) * (latlng.lat - a.lat) / (b.lat - a.lat) + a.lng) {
                inside = !inside;
            }
        }
        return inside;
    }

    // The topmost area under a point, for the right-click menu.
    function areaAt(latlng) {
        var areas = areaLayers();
        for (var i = areas.length - 1; i >= 0; i--) {
            if (areaContains(areas[i], latlng)) return areas[i];
        }
        return null;
    }

    // Shift held during a drawing adds the area; one set from the coordinate field always replaces.
    var _drawing = false;
    var _addToSelection = false;
    var _boxZoomWasOn = false;
    function noteShift(e) {
        if (_drawing && e.shiftKey) _addToSelection = true;
    }
    document.addEventListener('keydown', function (e) {
        if (e.key === 'Shift') noteShift({ shiftKey: true });
    }, true);
    map.getContainer().addEventListener('mousedown', noteShift, true);
    map.getContainer().addEventListener('click', noteShift, true);
    map.on('draw:drawstart', function () {
        _drawing = true;
        _addToSelection = false;
        // Shift-drag is also Leaflet's box zoom, which would zoom into the drawing.
        _boxZoomWasOn = !!(map.boxZoom && map.boxZoom.enabled());
        if (_boxZoomWasOn) map.boxZoom.disable();
    });
    map.on('draw:drawstop', function () {
        _drawing = false;
        if (_boxZoomWasOn) map.boxZoom.enable();
    });
    // What the legend (meld-features.js) may show: the selection, a box being drawn, and the boxes of
    // land the world holds (the One World preview).
    window.arnisLegendState = function () {
        return {
            selection: drawnItems.getLayers().length > 0,
            drawing: _drawing,
            rect: Boolean(shapesRectLayer),
            held: Boolean(worldOverlayEnabled && worldOverlayData && worldOverlayData.areas),
            picked: cellPick.size > 0
        };
    };

    // One World and live mode generate rectangles only (with Advanced features off).
    var _shapesLocked = false;

    // Custom icon for drawn markers
    var customMarkerIcon = L.icon({
        iconUrl: 'images/marker-icon.png',
        iconSize: [20, 20],
        iconAnchor: [10, 10],
        popupAnchor: [0, -10]
    });

    // Calculate geographic angle between two lat/lng points (in degrees)
    function calculateAngleGeo(latlng1, latlng2) {
        var lat1 = latlng1.lat * Math.PI / 180;
        var lat2 = latlng2.lat * Math.PI / 180;
        var dx = (latlng2.lng - latlng1.lng) * Math.cos((lat1 + lat2) / 2);
        var dy = latlng2.lat - latlng1.lat;
        var radians = Math.atan2(dy, dx);
        var degrees = radians * (180 / Math.PI);
        if (degrees < 0) degrees += 360;
        return degrees;
    }

    // Calculate the signed rotation needed to align to the nearest cardinal axis (0, 90, 180, 270)
    // Positive = clockwise on map, negative = counterclockwise
    function getRotationToNearestAxis(angle) {
        var axes = [0, 90, 180, 270, 360];
        var bestDiff = 360;
        for (var i = 0; i < axes.length; i++) {
            var diff = angle - axes[i];
            if (Math.abs(diff) < Math.abs(bestDiff)) bestDiff = diff;
        }
        return bestDiff;
    }

    drawControl = new L.Control.Draw({
        edit: {
            featureGroup: drawnItems,
            // No edit mode: the bbox rectangle has always-on drag handles instead
            edit: false
        },
        draw: {
            rectangle: {
                shapeOptions: {
                    color: '#fecc44',
                    opacity: 0.8,
                    weight: 3,
                    fillColor: '#fecc44',
                    fillOpacity: 0.1,
                    dashArray: window.arnisPieceStyle.selectionDash,
                    lineCap: 'round',
                    lineJoin: 'round'
                },
                repeatMode: false
            },
            polyline: false,
            polygon: {
                allowIntersection: false,
                showArea: false,
                drawError: { color: '#fa7878', timeout: 2500 },
                shapeOptions: {
                    color: '#fecc44',
                    opacity: 0.8,
                    weight: 3,
                    fillColor: '#fecc44',
                    fillOpacity: 0.1,
                    dashArray: window.arnisPieceStyle.selectionDash,
                    lineCap: 'round',
                    lineJoin: 'round'
                },
                repeatMode: false
            },
            circle: false,
            circlemarker: false,
            marker: {
                icon: customMarkerIcon
            }
        }
    });
    map.addControl(drawControl);

    // Leaflet.draw lists the shape tool first; it belongs after the rectangle.
    (function placeShapeTool() {
        var bar = document.querySelector('.leaflet-draw-toolbar.leaflet-draw-toolbar-top');
        var rect = bar && bar.querySelector('.leaflet-draw-draw-rectangle');
        var shape = bar && bar.querySelector('.leaflet-draw-draw-polygon');
        if (rect && shape) bar.insertBefore(shape, rect.nextSibling);
    })();

    // Leaflet.draw's own hints while a shape is being drawn.
    function localizeDrawHints() {
        var h = L.drawLocal.draw.handlers;
        h.polygon.tooltip.start = mapText('map_shape_start', 'Click to place the first corner.');
        h.polygon.tooltip.cont = mapText('map_shape_continue', 'Click to place the next corner.');
        h.polygon.tooltip.end = mapText('map_shape_end', 'Click the first corner to close the shape.');
        h.polyline.error = mapText('map_shape_cross', 'The outline cannot cross itself.');
        // The shape tool copied the message when it was made, so it is set there too.
        var mode = drawControl && drawControl._toolbars && drawControl._toolbars.draw &&
            drawControl._toolbars.draw._modes.polygon;
        if (mode) mode.handler.options.drawError.message = h.polyline.error;
        // Read each time a tool starts, so a language change reaches them too.
        var t = L.drawLocal.draw.toolbar;
        t.finish.title = t.finish.text = mapText('map_shape_finish', 'Finish');
        t.undo.title = t.undo.text = mapText('map_shape_undo', 'Delete last point');
        t.actions.title = t.actions.text = mapText('map_draw_cancel', 'Cancel');
    }
    localizeDrawHints();

    // ========== Toolbar labels and tooltips ==========
    // Every button says what it is and what it does, and the ones that depend
    // on the selection say why they cannot be used yet. The text is shown in
    // the map's own tooltip beside the button: the browser's title bubble is
    // slow, plain and was English only. The state lives in this file, so each
    // button is described from it rather than from a string set elsewhere.
    var BODY_METRES_PER_BLOCK = { moon: 200, mars: 500 };
    var toolTip = document.createElement('div');
    toolTip.className = 'map-tool-tip';
    toolTip.setAttribute('role', 'tooltip');
    map.getContainer().appendChild(toolTip);
    var toolTipFor = null;

    function describeTool(btn) {
        var c = btn.classList;
        var addHint = mapText('map_tool_add_hint', 'Hold Shift while drawing to add the shape to the selection.');
        if (c.contains('leaflet-draw-draw-rectangle')) {
            return {
                name: mapText('map_tool_area', 'Select area'),
                desc: mapText('map_tool_area_desc', 'Drag over the map to choose what gets generated.') + (_shapesLocked ? '' : ' ' + addHint)
            };
        }
        if (c.contains('leaflet-draw-draw-polygon')) {
            return {
                name: mapText('map_tool_shape', 'Draw a shape'),
                desc: (_shapesMode === 'rect' ? '' : (_shapesMode === 'pieces'
                    ? mapText('map_tool_shape_pieces_desc', 'A cell the shape touches is built in full.')
                    : mapText('map_tool_shape_desc', 'Click around the area to generate exactly that outline.')) + ' ') + addHint,
                note: _shapesLocked ? mapText('map_tool_shape_rectangles_only', 'One World generates rectangles only.')
                    : _shapesMode === 'rect' ? mapText('map_tool_shape_one_world', 'One World builds the rectangle around the shape. Building in cells keeps the shape itself.') : ''
            };
        }
        if (c.contains('leaflet-draw-draw-polyline')) {
            return {
                name: mapText('map_tool_rotation', 'Rotation angle'),
                desc: mapText('map_tool_rotation_desc', 'Draw a line along a street to line the world up with it.'),
                note: _rotationLocked ? mapText('map_tool_rotation_one_world', 'Off in One World mode.') : ''
            };
        }
        if (c.contains('leaflet-draw-draw-cells')) {
            return {
                name: mapText('map_tool_cells', 'Select cells'),
                desc: mapText('map_tool_cells_desc', 'Click a cell or drag a box. Shift or Ctrl adds. Right-click for actions. Esc clears.'),
                note: cellPick.size ? cellCount(cellPick.size) : ''
            };
        }
        if (c.contains('leaflet-draw-draw-marker')) {
            return {
                name: mapText('map_tool_spawn', 'Spawn point'),
                desc: mapText('map_tool_spawn_desc', 'Click where players start in the world.')
            };
        }
        if (c.contains('leaflet-draw-edit-remove')) {
            return {
                name: mapText('map_tool_clear', 'Clear selection'),
                desc: mapText('map_tool_clear_desc', 'Removes the selected area and the spawn point.'),
                note: !drawnItems.getLayers().length ? mapText('map_tool_clear_empty', 'Nothing is selected yet.')
                    : window.arnisMeldOn ? mapText('map_tool_clear_hold', 'Hold to clear') : ''
            };
        }
        if (btn.id === 'world-preview-btn') {
            return {
                name: mapText('map_tool_world_preview', 'World preview'),
                desc: mapText('map_tool_world_preview_desc', 'Lays the generated world over the map.'),
                note: worldPreviewAvailable ? '' : mapText('map_tool_world_preview_unavailable', 'Available after generating a world.')
            };
        }
        if (btn.id === 'terrain-preview-btn') {
            var terrainNote = '';
            if (c.contains('disabled')) {
                terrainNote = currentBody !== 'earth'
                    ? mapText('map_tool_terrain_earth_only', 'Only available on Earth.')
                    : mapText('map_tool_terrain_select', 'Select an area of up to 500 km² first.');
            }
            return {
                name: mapText('map_tool_terrain', '3D terrain preview'),
                desc: mapText('map_tool_terrain_desc', "Shows the selected area's terrain in 3D."),
                note: terrainNote
            };
        }
        if (btn.id === 'body-toggle-btn') {
            var bodyName = mapText('body_' + currentBody,
                currentBody.charAt(0).toUpperCase() + currentBody.slice(1));
            var metres = BODY_METRES_PER_BLOCK[currentBody];
            return {
                name: mapText('celestial_body', 'World') + ': ' + bodyName,
                desc: mapText('map_tool_body_desc', 'Click to switch between Earth, the Moon and Mars.'),
                note: metres
                    ? mapText('map_tool_body_scale', 'NASA terrain only, 1 block = {m} m.').replace('{m}', metres)
                    : ''
            };
        }
        return null;
    }

    function showToolTip(btn) {
        var d = describeTool(btn);
        if (!d) {
            hideToolTip();
            return;
        }
        toolTip.textContent = '';
        var name = document.createElement('strong');
        name.textContent = d.name;
        var desc = document.createElement('span');
        desc.textContent = d.desc;
        toolTip.appendChild(name);
        toolTip.appendChild(desc);
        if (d.note) {
            var note = document.createElement('span');
            note.className = 'map-tool-tip-note';
            note.textContent = d.note;
            toolTip.appendChild(note);
        }
        var mapRect = map.getContainer().getBoundingClientRect();
        var r = btn.getBoundingClientRect();
        // Centred on the button, but kept inside the map: the top button sits
        // close to its edge, and the map clips what hangs over.
        var half = toolTip.offsetHeight / 2;
        var centre = r.top - mapRect.top + r.height / 2;
        centre = Math.max(half + 6, Math.min(centre, mapRect.height - half - 6));
        toolTip.style.left = (r.right - mapRect.left + 10) + 'px';
        toolTip.style.top = centre + 'px';
        toolTip.classList.add('is-visible');
        toolTipFor = btn;
    }

    function hideToolTip() {
        toolTip.classList.remove('is-visible');
        toolTipFor = null;
    }

    // The accessible name follows the same description, and an open tooltip
    // follows a state change under the pointer (e.g. the selection cleared).
    function refreshToolLabel(btn) {
        if (!btn) return;
        btn.removeAttribute('title');
        var d = describeTool(btn);
        if (d) btn.setAttribute('aria-label', d.note ? d.name + '. ' + d.note : d.name);
        if (btn === toolTipFor) showToolTip(btn);
    }
    window.refreshMapToolLabel = refreshToolLabel;

    // Whether this user has ever selected an area. The hint at the bottom of
    // the map and the outline on the area tool teach that one step; once it
    // has been done they stay away, across restarts too. Storage that throws
    // (blocked, private) simply keeps teaching. The key is written out in both
    // functions: they are hoisted, and may run before a variable here is set.
    function areaToolLearned() {
        try {
            return localStorage.getItem('arnis-area-selected-once') === 'true';
        } catch (e) {
            return false;
        }
    }

    function markAreaToolLearned() {
        try {
            localStorage.setItem('arnis-area-selected-once', 'true');
        } catch (e) {
            // Not remembered; the hint returns next time, which is harmless.
        }
    }

    // Until the first area is selected, the tool that draws one is outlined.
    function refreshAreaToolAttention() {
        var areaBtn = document.querySelector('.leaflet-draw-toolbar .leaflet-draw-draw-rectangle');
        if (!areaBtn) return;
        areaBtn.classList.toggle('map-tool-attention', !areaLayers().length && !areaToolLearned());
    }
    drawnItems.on('layeradd layerremove', refreshAreaToolAttention);

    // Every label again, for a language change. Also the search placeholder,
    // the other piece of text on the map that is always visible.
    function refreshMapToolLabels() {
        localizeDrawHints();
        document.querySelectorAll('.leaflet-draw-toolbar a').forEach(refreshToolLabel);
        var search = document.getElementById('city-search');
        if (search) search.placeholder = mapText('map_search_placeholder', 'Search for a city...');
    }
    window.refreshMapToolLabels = refreshMapToolLabels;

    (function bindToolTips() {
        var drawRoot = document.querySelector('.leaflet-draw');
        if (!drawRoot) return;
        var buttonOf = function (target) {
            return target && target.closest ? target.closest('.leaflet-draw-toolbar a') : null;
        };
        drawRoot.addEventListener('mouseover', function (e) {
            var btn = buttonOf(e.target);
            if (btn && btn !== toolTipFor) showToolTip(btn);
        });
        drawRoot.addEventListener('mouseout', function (e) {
            var btn = buttonOf(e.target);
            if (btn && !(e.relatedTarget && btn.contains(e.relatedTarget))) hideToolTip();
        });
        drawRoot.addEventListener('focusin', function (e) {
            var btn = buttonOf(e.target);
            if (btn) showToolTip(btn);
        });
        drawRoot.addEventListener('focusout', hideToolTip);
        // Out of the way once a tool is picked, so it does not cover the map
        // about to be drawn on.
        drawRoot.addEventListener('mousedown', hideToolTip);
    })();

    // ========== Custom Angle Line Tool ==========
    // A simple 2-click tool: click start point, click end point, done.
    // Uses a transparent overlay to capture clicks even on top of drawn layers.
    var _angleLine = null;
    var _angleStartLatLng = null;
    var _angleToolActive = false;
    var _angleToolBtn = null;
    var _angleOverlay = null;        // transparent click-capture div

    function startAngleTool() {
        stopAngleTool();
        _angleToolActive = true;

        // Create a transparent overlay over the map to capture all clicks
        // (otherwise clicks on the bbox rectangle get swallowed by the layer).
        // z-index 700: above the map panes but below the leaflet controls, so
        // the toolbar stays clickable while the angle tool is active.
        _angleOverlay = document.createElement('div');
        _angleOverlay.style.cssText = 'position:absolute;top:0;left:0;width:100%;height:100%;z-index:700;cursor:crosshair;';
        map.getContainer().appendChild(_angleOverlay);

        _angleOverlay.addEventListener('click', _onAngleOverlayClick);
        _angleOverlay.addEventListener('mousemove', _onAngleOverlayMouseMove);
        // The toolbar stays clickable above the overlay; a click on any other
        // tool cancels the measurement instead of mixing modes.
        document.addEventListener('click', _onAngleToolbarClick, true);
    }

    function _onAngleToolbarClick(e) {
        if (!_angleToolActive) return;
        var toolbar = e.target.closest && e.target.closest('.leaflet-draw-toolbar');
        if (toolbar && (!_angleToolBtn || !_angleToolBtn.contains(e.target))) {
            stopAngleTool();
        }
    }

    function stopAngleTool() {
        _angleToolActive = false;
        _angleStartLatLng = null;
        document.removeEventListener('click', _onAngleToolbarClick, true);
        if (_angleOverlay) {
            _angleOverlay.removeEventListener('click', _onAngleOverlayClick);
            _angleOverlay.removeEventListener('mousemove', _onAngleOverlayMouseMove);
            _angleOverlay.parentNode && _angleOverlay.parentNode.removeChild(_angleOverlay);
            _angleOverlay = null;
        }
        if (_angleLine) {
            map.removeLayer(_angleLine);
            _angleLine = null;
        }
        if (_angleToolBtn) {
            L.DomUtil.removeClass(_angleToolBtn, 'leaflet-draw-toolbar-button-enabled');
        }
    }

    function _overlayEventToLatLng(e) {
        var rect = map.getContainer().getBoundingClientRect();
        var point = L.point(e.clientX - rect.left, e.clientY - rect.top);
        return map.containerPointToLatLng(point);
    }

    function _onAngleOverlayClick(e) {
        var latlng = _overlayEventToLatLng(e);

        if (!_angleStartLatLng) {
            // First click — place start point
            _angleStartLatLng = latlng;
            _angleLine = L.polyline([_angleStartLatLng, _angleStartLatLng], {
                color: '#00aaff',
                weight: 3,
                dashArray: '5, 5'
            }).addTo(map);
        } else {
            // Second click — finish
            _angleLine.setLatLngs([_angleStartLatLng, latlng]);

            var angle = calculateAngleGeo(_angleStartLatLng, latlng);
            var rotation = getRotationToNearestAxis(angle);
            var rotationValue = parseFloat(rotation.toFixed(2));

            window.parent.postMessage({
                type: 'angleMeasured',
                angle: rotationValue
            }, '*');

            showRotationToast('Rotation angle set to ' + rotationValue + '\u00B0 (see settings)');

            // Keep the line visible briefly, then remove
            var lineRef = _angleLine;
            _angleLine = null;
            setTimeout(function() {
                if (lineRef) map.removeLayer(lineRef);
            }, 1500);

            stopAngleTool();
        }
    }

    function _onAngleOverlayMouseMove(e) {
        if (_angleLine && _angleStartLatLng) {
            _angleLine.setLatLngs([_angleStartLatLng, _overlayEventToLatLng(e)]);
        }
    }

    var _rotationLocked = false;

    function setShapesLocked(locked) {
        _shapesLocked = locked;
        var btn = document.querySelector('.leaflet-draw-toolbar .leaflet-draw-draw-polygon');
        if (!btn) return;
        if (locked && drawControl._toolbars.draw._activeMode &&
            drawControl._toolbars.draw._activeMode.handler.type === 'polygon') {
            drawControl._toolbars.draw._activeMode.handler.disable();
        }
        btn.classList.toggle('leaflet-disabled', locked);
        btn.setAttribute('aria-disabled', locked ? 'true' : 'false');
        refreshToolLabel(btn);
        var rect = document.querySelector('.leaflet-draw-toolbar .leaflet-draw-draw-rectangle');
        if (rect) refreshToolLabel(rect);
    }

    // Stops a locked tool before Leaflet.draw's own click handler starts it.
    (function guardShapeTool() {
        var btn = document.querySelector('.leaflet-draw-toolbar .leaflet-draw-draw-polygon');
        if (!btn) return;
        btn.addEventListener('click', function (e) {
            if (!_shapesLocked) return;
            e.preventDefault();
            e.stopImmediatePropagation();
        }, true);
    })();

    function setRotationLocked(locked) {
        _rotationLocked = locked;
        if (locked && _angleToolActive) stopAngleTool();
        if (_angleToolBtn) {
            _angleToolBtn.classList.toggle('leaflet-disabled', locked);
            _angleToolBtn.setAttribute('aria-disabled', locked ? 'true' : 'false');
            refreshToolLabel(_angleToolBtn);
        }
    }

    // Inject the angle tool button into the top draw toolbar (alongside rectangle & marker)
    (function addAngleToolButton() {
        var drawToolbar = document.querySelector('.leaflet-draw-toolbar.leaflet-draw-toolbar-top');
        if (!drawToolbar) return;

        var btn = L.DomUtil.create('a', 'leaflet-draw-draw-polyline');
        btn.href = '#';

        L.DomEvent
            .on(btn, 'click', L.DomEvent.stopPropagation)
            .on(btn, 'mousedown', L.DomEvent.stopPropagation)
            .on(btn, 'dblclick', L.DomEvent.stopPropagation)
            .on(btn, 'click', L.DomEvent.preventDefault)
            .on(btn, 'click', function() {
                if (_rotationLocked) return;
                if (_angleToolActive) {
                    stopAngleTool();
                } else {
                    startAngleTool();
                    L.DomUtil.addClass(btn, 'leaflet-draw-toolbar-button-enabled');
                }
            });

        _angleToolBtn = btn;
        if (_rotationLocked) setRotationLocked(true);

        // Insert before the marker (spawn) button so it's: rectangle | angle | marker
        var markerBtn = drawToolbar.querySelector('.leaflet-draw-draw-marker');
        if (markerBtn) {
            drawToolbar.insertBefore(btn, markerBtn);
        } else {
            drawToolbar.appendChild(btn);
        }
    })();

    // The select tool for cells: in the top toolbar with the draw tools; shown only while the map has cells.
    (function addCellToolButton() {
        var drawToolbar = document.querySelector('.leaflet-draw-toolbar.leaflet-draw-toolbar-top');
        if (!drawToolbar) return;
        var btn = L.DomUtil.create('a', 'leaflet-draw-draw-cells', drawToolbar);
        btn.href = '#';
        btn.setAttribute('role', 'button');
        btn.setAttribute('aria-pressed', 'false');
        btn.style.display = 'none';
        L.DomEvent
            .on(btn, 'click mousedown dblclick', L.DomEvent.stopPropagation)
            .on(btn, 'click', L.DomEvent.preventDefault)
            .on(btn, 'click', function() { setCellTool(!cellTool); });
        cellBtn = btn;
        refreshToolLabel(btn);
    })();

    // Whether the map has cells to pick: the plan's or a job's own, not the grey ones around.
    function syncCellTool() {
        if (!cellBtn) return;
        var any = lastPieces.some(window.arnisCells.pickable);
        cellBtn.style.display = any ? '' : 'none';
        if (!any) setCellTool(false);
        var ids = {};
        lastPieces.forEach(function(p) { ids[p.i] = true; });
        var size = cellPick.size;
        cellPick.forEach(function(i) { if (!ids[i]) cellPick.delete(i); });
        if (cellPick.size !== size) window.parent.postMessage({ type: 'cellsPicked', n: cellPick.size }, '*');
    }

    function setCellTool(on) {
        if (on === cellTool) return;
        cellTool = on;
        cellBtn.classList.toggle('leaflet-draw-toolbar-button-enabled', on);
        cellBtn.setAttribute('aria-pressed', String(on));
        map.getContainer().classList.toggle('cell-tool', on);
        if (!on) pickCells(new Set());
        refreshToolLabel(cellBtn);
    }

    // The picked cells change: draw them, say so to the window (the legend), refresh the tool's note.
    function pickCells(set) {
        if (set.size === cellPick.size && Array.from(set).every(function(i) { return cellPick.has(i); })) return;
        cellPick = set;
        drawPieceOutlines(lastPieces);
        window.parent.postMessage({ type: 'cellsPicked', n: set.size }, '*');
        refreshToolLabel(cellBtn);
    }

    // A draw tool and the select tool do not run together.
    map.on('draw:drawstart', function() { setCellTool(false); });

    // Click picks a cell, a drag a box of cells (by plan geometry, so cells not drawn at this zoom
    // count); Shift or Ctrl adds. Caught before the map sees the press, so the map does not pan.
    map.getContainer().addEventListener('mousedown', function(e) {
        if (!cellTool || e.button !== 0 || e.target.closest('.leaflet-control-container, .map-tool-tip, .piece-tip')) return;
        e.stopPropagation();
        e.preventDefault();
        cellDrag = { x: e.clientX, y: e.clientY, from: map.mouseEventToLatLng(e), add: e.shiftKey || e.ctrlKey || e.metaKey, moved: false };
    }, true);
    document.addEventListener('mousemove', function(e) {
        if (!cellDrag) return;
        if (!cellDrag.moved && Math.hypot(e.clientX - cellDrag.x, e.clientY - cellDrag.y) < 4) return;
        cellDrag.moved = true;
        var box = L.latLngBounds(cellDrag.from, map.mouseEventToLatLng(e));
        if (cellBoxLayer) cellBoxLayer.setBounds(box);
        else cellBoxLayer = L.rectangle(box, { color: PIECE_COLORS.picked, weight: 1, dashArray: '4 4', fillOpacity: 0.1, interactive: false }).addTo(map);
    });
    document.addEventListener('mouseup', function(e) {
        if (!cellDrag) return;
        var drag = cellDrag;
        cellDrag = null;
        if (cellBoxLayer) { map.removeLayer(cellBoxLayer); cellBoxLayer = null; }
        var to = map.mouseEventToLatLng(e);
        var hits = drag.moved
            ? window.arnisCells.inBox(lastPieces, [drag.from.lat, drag.from.lng, to.lat, to.lng])
            : [window.arnisCells.at(lastPieces, drag.from.lat, drag.from.lng)].filter(function(i) { return i >= 0; });
        pickCells(window.arnisCells.pick(cellPick, hits, drag.add));
    });
    document.addEventListener('keydown', function(e) {
        if (e.key === 'Escape') hideContextMenu();
        pickCells(window.arnisCells.key(e, cellPick));
    });

    // Hint at the bottom-center of the map, shown until the first area is
    // selected (see areaToolLearned)
    var hintDiv = document.createElement('div');
    hintDiv.className = 'bbox-hint-overlay';
    // This script runs inside the map iframe, and the locale is loaded by the
    // top document, so it is read through the parent (same origin). Exposed on
    // the window so main.js can call it again after a language change.
    function renderBboxHint() {
        var loc = window.localization
            || (window.parent && window.parent !== window && window.parent.localization)
            || null;
        var hintText = (loc && loc.bbox_hint) || 'Use the {tool} tool to draw an area';
        // {tool} is where the icon goes, so a translation can put it wherever
        // its own grammar wants it. Matching an English phrase here would have
        // dropped the icon in every other language.
        hintDiv.innerHTML = hintText.split('{tool}').join('<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" style="vertical-align: -2px; opacity: 0.85;"><rect x="5" y="5" width="14" height="14" stroke-width="1.4"></rect><g fill="currentColor" stroke="none"><rect x="3" y="3" width="4" height="4"></rect><rect x="17" y="3" width="4" height="4"></rect><rect x="3" y="17" width="4" height="4"></rect><rect x="17" y="17" width="4" height="4"></rect></g></svg>');
    }
    renderBboxHint();
    window.renderBboxHint = renderBboxHint;
    if (areaToolLearned()) hintDiv.style.display = 'none';
    map.getContainer().appendChild(hintDiv);

    // Add world preview button to the edit toolbar after drawControl is added
    addWorldPreviewToEditToolbar();

    // One-click bbox delete: replace leaflet.draw's enter-mode -> click shape ->
    // save flow on the trash button (the app only ever has one selection).
    (function makeDeleteOneClick() {
        var oldBtn = document.querySelector('.leaflet-draw-edit-remove');
        if (!oldBtn || !oldBtn.parentNode) return;
        var btn = oldBtn.cloneNode(true); // drops leaflet.draw's mode listeners
        oldBtn.parentNode.replaceChild(btn, oldBtn);

        function syncState() {
            var has = drawnItems.getLayers().length > 0;
            btn.classList.toggle('leaflet-disabled', !has);
            refreshToolLabel(btn);
        }
        drawnItems.on('layeradd layerremove', syncState);
        syncState();

        function clearAll() {
            var removed = L.layerGroup();
            drawnItems.eachLayer(function (l) { removed.addLayer(l); });
            if (removed.getLayers().length === 0) return;
            // The existing draw:deleted handler removes the layers, resets
            // the bounds, notifies the parent and refreshes the handles.
            map.fire('draw:deleted', { layers: removed });
        }
        // A click clears; with Advanced features the button has to be held (arnisClearHoldMs),
        // with a fill growing on it, and letting go early cancels.
        L.DomEvent
            .on(btn, 'mousedown dblclick', L.DomEvent.stopPropagation)
            .on(btn, 'click', L.DomEvent.stop)
            .on(btn, 'click', function () { if (!window.arnisMeldOn) clearAll(); });
        window.arnisHold(btn, clearAll, window.arnisClearHoldMs, function () {
            return !window.arnisMeldOn || btn.classList.contains('leaflet-disabled');
        });
    })();

    // Terrain preview button: usable once the selected bbox fits the 3D
    // preview size gate; clicking asks the parent to render the mini preview.
    (function addTerrainPreviewButton() {
        var editToolbar = document.querySelector('.leaflet-draw-toolbar:not(.leaflet-draw-toolbar-top)');
        if (!editToolbar) {
            var anchor = document.querySelector('.leaflet-draw-edit-remove');
            if (anchor) editToolbar = anchor.parentElement;
        }
        if (!editToolbar) return;

        var btn = document.createElement('a');
        btn.className = 'leaflet-draw-edit-terrain disabled';
        btn.href = '#';
        btn.id = 'terrain-preview-btn';

        btn.addEventListener('click', function (e) {
            e.preventDefault();
            e.stopPropagation();
            if (btn.classList.contains('disabled')) return;
            // Field is deliberately NOT named bboxText: the parent's message
            // handler treats any bboxText-bearing message as a selection update.
            window.parent.postMessage({
                type: 'renderTerrainPreview',
                previewBBoxText: document.getElementById('boxbounds').textContent
            }, '*');
        });

        editToolbar.appendChild(btn);
        window._terrainPreviewBtn = btn;
        updateTerrainPreviewButton();
    })();

    // World toggle: its own toolbar group below the edit tools, cycling
    // Earth -> Moon -> Mars. The map is the only place the body is picked.
    (function addBodyToggleButton() {
        var drawContainer = document.querySelector('.leaflet-draw');
        if (!drawContainer) return;

        var section = L.DomUtil.create('div', 'leaflet-draw-section', drawContainer);
        var bar = L.DomUtil.create('div', 'leaflet-draw-toolbar leaflet-bar', section);
        var btn = L.DomUtil.create('a', 'leaflet-draw-edit-body', bar);
        btn.href = '#';
        btn.id = 'body-toggle-btn';

        L.DomEvent
            .on(btn, 'mousedown dblclick', L.DomEvent.stopPropagation)
            .on(btn, 'click', L.DomEvent.stop)
            .on(btn, 'click', function () {
                // A live session plays Earth until it ends.
                if (liveActive) return;
                var i = BODY_CYCLE.indexOf(currentBody);
                changeBody(BODY_CYCLE[(i + 1) % BODY_CYCLE.length]);
                // The parent gates Earth-only settings off this.
                window.parent.postMessage({ type: 'bodyChanged', body: currentBody }, '*');
            });

        _bodyToggleBtn = btn;
        syncBodyToggleButton();
    })();

    // Leaflet.draw's own buttons came with English titles; every button is
    // labelled from its description now.
    refreshMapToolLabels();
    refreshAreaToolAttention();
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
        // bbox.css fades it while pieces build.
        className: 'bbox-selection',
        color: '#fecc44',
        opacity: 1.0,
        weight: 3,
        fill: '#fecc44',
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

    // ========== Always-on area handles ==========
    // Rectangles get corner handles, shapes Leaflet.draw's vertex editing; both a centre move handle.
    var _bboxHandles = [];

    // The bounds frame shows over a single rectangle only; with shapes it would look like an area.
    function styleBoundsFrame(areas) {
        var plain = areas.length === 1 && areas[0] instanceof L.Rectangle;
        bounds.setStyle(plain ? { opacity: 1.0, fillOpacity: 0.2 } : { opacity: 0, fillOpacity: 0 });
        // Invisible, it must not take the pointer from the shapes under it either.
        if (bounds._path) L.DomUtil[plain ? 'addClass' : 'removeClass'](bounds._path, 'leaflet-interactive');
    }

    // Bounds, the coordinate fields and the parent all follow the areas.
    function syncSelection() {
        var areas = areaLayers();
        if (areas.length) {
            var union = new L.LatLngBounds();
            areas.forEach(function (l) { union.extend(l.getBounds()); });
            bounds.setBounds(union);
        } else {
            bounds.setBounds(new L.LatLngBounds([0.0, 0.0], [0.0, 0.0]));
        }
        styleBoundsFrame(areas);
        if (selectionShapes() && !fitOneSide(areas)) {
            showRotationToast(mapText('map_shape_date_line', 'Draw the area on one side of the 180° meridian.'));
        } else if (tooFarApart(areas)) {
            // Moved apart by a handle: kept, but said.
            showRotationToast(mapText('map_shape_too_far', 'Too far from the other areas. Everything between them would be generated too. Make it a world of its own.'));
        }
        $('#boxbounds').text(formatBounds(bounds.getBounds(), '4326'));
        $('#boxboundsmerc').text(formatBounds(bounds.getBounds(), currentproj));
        notifyBboxUpdate();
    }

    function clearBboxHandles() {
        _bboxHandles.forEach(function (h) { map.removeLayer(h); });
        _bboxHandles = [];
        sizeLabel = null;
    }

    function addMoveHandle(layer, center, moveTo) {
        var moveIcon = L.divIcon({
            className: 'bbox-handle bbox-handle-move',
            iconSize: [16, 16],
            iconAnchor: [8, 8]
        });
        var mover = L.marker(center, { icon: moveIcon, draggable: true, zIndexOffset: 2000 });
        var startCenter = null;
        mover.on('dragstart', function (ev) {
            startCenter = ev.target.getLatLng();
            moveTo.start();
        });
        mover.on('drag', function (ev) {
            var cur = ev.target.getLatLng();
            moveTo.by(cur.lat - startCenter.lat, cur.lng - startCenter.lng);
        });
        mover.on('dragend', function () {
            moveTo.end();
            syncSelection();
            refreshBboxHandles();
        });
        mover.addTo(map);
        _bboxHandles.push(mover);
    }

    // Snap to Pieces on: a dragged selection changes in whole regions (a region is 512 blocks,
    // 512 / scale metres), so what is drawn is what the build covers.
    function regionDegrees(lat) {
        var side = 512 / (window.arnisWorldScale || 1);
        return [side / map.distance([lat, 0], [lat + 1, 0]), side / map.distance([lat, 0], [lat, 1])];
    }
    function snapShift(dLat, dLng, lat) {
        if (!window.arnisMeldOn || !window.arnisSnapOn) return [dLat, dLng];
        var r = regionDegrees(lat);
        return [arnisSnap.shift(dLat, r[0]), arnisSnap.shift(dLng, r[1])];
    }
    // `p` dragged against the fixed edge or corner (a null axis is not constrained).
    function snapPoint(fixedLat, fixedLng, p) {
        if (!window.arnisMeldOn || !window.arnisSnapOn) return p;
        var r = regionDegrees(fixedLat === null ? p.lat : fixedLat);
        return L.latLng(
            fixedLat === null ? p.lat : fixedLat + arnisSnap.span(p.lat - fixedLat, r[0]),
            fixedLng === null ? p.lng : fixedLng + arnisSnap.span(p.lng - fixedLng, r[1]));
    }

    function addRectHandles(rect) {
        var b = rect.getBounds();
        var corners = [
            { get: 'getNorthWest', opp: 'getSouthEast', cls: 'nwse' },
            { get: 'getNorthEast', opp: 'getSouthWest', cls: 'nesw' },
            { get: 'getSouthEast', opp: 'getNorthWest', cls: 'nwse' },
            { get: 'getSouthWest', opp: 'getNorthEast', cls: 'nesw' }
        ];

        corners.forEach(function (c) {
            var icon = L.divIcon({
                className: 'bbox-handle bbox-handle-' + c.cls,
                iconSize: [12, 12],
                iconAnchor: [6, 6]
            });
            var marker = L.marker(b[c.get](), { icon: icon, draggable: true, zIndexOffset: 2000 });
            var fixedCorner = null;
            marker.on('dragstart', function () {
                fixedCorner = rect.getBounds()[c.opp]();
            });
            marker.on('drag', function (ev) {
                rect.setBounds(new L.LatLngBounds(fixedCorner, snapPoint(fixedCorner.lat, fixedCorner.lng, ev.target.getLatLng())));
                liveSelection(rect);
            });
            marker.on('dragend', function () {
                syncSelection();
                refreshBboxHandles();
            });
            marker.addTo(map);
            _bboxHandles.push(marker);
        });

        var startB = null;
        addMoveHandle(rect, b.getCenter(), {
            start: function () { startB = rect.getBounds(); },
            by: function (dLat, dLng) {
                var d = snapShift(dLat, dLng, startB.getCenter().lat);
                dLat = d[0]; dLng = d[1];
                rect.setBounds(new L.LatLngBounds(
                    [startB.getSouth() + dLat, startB.getWest() + dLng],
                    [startB.getNorth() + dLat, startB.getEast() + dLng]
                ));
                liveSelection(rect);
            },
            end: function () {}
        });
    }

    function addShapeHandles(shape) {
        if (shape.editing && !shape.editing.enabled()) shape.editing.enable();
        var startRing = null;
        addMoveHandle(shape, shape.getBounds().getCenter(), {
            start: function () {
                startRing = ringOf(shape).map(function (p) { return L.latLng(p.lat, p.lng); });
                // The corner handles would stay behind while the outline moves.
                if (shape.editing) shape.editing.disable();
            },
            by: function (dLat, dLng) {
                shape.setLatLngs(startRing.map(function (p) {
                    return L.latLng(p.lat + dLat, p.lng + dLng);
                }));
            },
            end: function () {
                if (!shape.editing) return;
                // setLatLngs swapped the corner array; the editor would keep handles on the old one.
                shape.editing.latlngs = [shape._latlngs];
                shape.editing.enable();
            }
        });
    }

    function refreshBboxHandles() {
        clearBboxHandles();
        areaLayers().forEach(function (layer) {
            if (layer instanceof L.Rectangle) addRectHandles(layer);
            else addShapeHandles(layer);
        });
        if (window.arnisMeldOn && _getBboxRect()) meldHandles(_getBboxRect());
    }

    // ---- Advanced features: edge handles, move from inside, a size readout, a live plan ----

    // Each edge's midpoint moves that edge alone.
    // [edge, the opposite edge, axis]: the axis is the one the edge moves along.
    var EDGES = [['North', 'South', 'ns'], ['South', 'North', 'ns'], ['East', 'West', 'ew'], ['West', 'East', 'ew']].map(function (d) {
        var lat = d[2] === 'ns';
        return {
            cls: d[2],
            at: function (b) { var v = b['get' + d[0]](); return lat ? [v, b.getCenter().lng] : [b.getCenter().lat, v]; },
            set: function (b, p) {
                var o = { South: b.getSouth(), West: b.getWest(), North: b.getNorth(), East: b.getEast() };
                o[d[0]] = lat ? p.lat : p.lng;
                return [[o.South, o.West], [o.North, o.East]];
            },
            fix: function (b) { var v = b['get' + d[1]](); return lat ? [v, null] : [null, v]; }
        };
    });
    var sizeLabel = null;
    var liveSync = 0;

    function meldHandles(rect) {
        var b = rect.getBounds();
        EDGES.forEach(function (e) {
            var icon = L.divIcon({ className: 'bbox-handle bbox-handle-' + e.cls, iconSize: [10, 10], iconAnchor: [5, 5] });
            var marker = L.marker(e.at(b), { icon: icon, draggable: true, zIndexOffset: 2000 });
            var start = null;
            marker.on('dragstart', function () { start = rect.getBounds(); });
            marker.on('drag', function (ev) {
                var f = e.fix(start);
                rect.setBounds(L.latLngBounds(e.set(start, snapPoint(f[0], f[1], ev.target.getLatLng()))));
                liveSelection(rect);
            });
            marker.on('dragend', function () {
                syncSelection();
                refreshBboxHandles();
            });
            marker.addTo(map);
            _bboxHandles.push(marker);
        });
    }

    // "3.2 × 2.1 km · about 42 regions" at the world scale the window last sent.
    function sizeText(b) {
        var mid = b.getCenter().lat;
        var w = map.distance([mid, b.getWest()], [mid, b.getEast()]);
        var h = map.distance([b.getSouth(), b.getWest()], [b.getNorth(), b.getWest()]);
        var side = 512 / (window.arnisWorldScale || 1);
        return mapText('map_selection_size', '{w} × {h} km · about {r} regions')
            .replace('{w}', (w / 1000).toFixed(1)).replace('{h}', (h / 1000).toFixed(1))
            .replace('{r}', Math.ceil(w / side) * Math.ceil(h / side));
    }

    // While a handle drags: the readout follows, and with Advanced features the window gets the
    // selection once a second (and at the end), so the planned pieces follow without a plan
    // request on every move.
    function liveSelection(rect) {
        if (!window.arnisMeldOn || _getBboxRect() !== rect) return;
        var b = rect.getBounds();
        // Shown while dragging only: at rest it would sit over the ring's piece numbers.
        if (!sizeLabel) {
            sizeLabel = L.tooltip({ permanent: true, direction: 'top', className: 'bbox-size', interactive: false, offset: [0, -6] });
            sizeLabel.setLatLng([b.getNorth(), b.getCenter().lng]).addTo(map);
            _bboxHandles.push(sizeLabel);
        }
        sizeLabel.setLatLng([b.getNorth(), b.getCenter().lng]).setContent(sizeText(b));
        var now = Date.now();
        if (now - liveSync < 1000) return;
        liveSync = now;
        syncSelection();
    }

    // Advanced features: a drag that starts inside the selection moves it, while all of it is on
    // screen; zoomed into a big one, the drag pans the map.
    var inside = null;
    map.on('mousedown', function (ev) {
        var rect = window.arnisMeldOn && _getBboxRect();
        if (!rect || !rect.getBounds().contains(ev.latlng) || ev.originalEvent.button !== 0) return;
        if (!map.getBounds().contains(rect.getBounds())) return;
        if (drawControl && drawControl._toolbars && (drawControl._toolbars.draw || {})._activeMode) return;
        inside = { from: ev.latlng, b: rect.getBounds(), rect: rect };
        map.dragging.disable();
    });
    map.on('mousemove', function (ev) {
        if (!inside) return;
        // The button went up where the map did not see it (another frame, outside the window).
        if (!(ev.originalEvent.buttons & 1)) return endMove();
        var b = inside.b;
        var d = snapShift(ev.latlng.lat - inside.from.lat, ev.latlng.lng - inside.from.lng, b.getCenter().lat);
        var dLat = d[0], dLng = d[1];
        inside.rect.setBounds(L.latLngBounds([b.getSouth() + dLat, b.getWest() + dLng], [b.getNorth() + dLat, b.getEast() + dLng]));
        if (!inside.moved) clearBboxHandles();
        inside.moved = true;
        liveSelection(inside.rect);
    });
    function endMove() {
        if (!inside) return;
        var moved = inside.moved;
        var rect = inside.rect;
        inside = null;
        map.dragging.enable();
        // A click (say on a failed piece, to retry it) moved nothing.
        if (!moved) return;
        syncSelection();
        refreshBboxHandles();
    }
    map.on('mouseup', endMove);
    document.addEventListener('mouseup', endMove);
    window.addEventListener('blur', endMove);

    // Show a brief toast notification on the map
    function showRotationToast(message) {
        // Remove any existing toast
        var existing = map.getContainer().querySelector('.rotation-toast');
        if (existing) existing.remove();

        var toast = document.createElement('div');
        toast.className = 'rotation-toast';
        toast.textContent = message;
        map.getContainer().appendChild(toast);

        setTimeout(function() {
            toast.classList.add('fade-out');
            setTimeout(function() { toast.remove(); }, 600);
        }, 6000);
    }

    map.on('draw:created', function (e) {
        var area = isArea(e.layer);
        var adding = area && _drawing && _addToSelection && !_shapesLocked && areaLayers().length > 0;

        // No map copy holds an area across the 180° meridian, nor areas on both sides of it.
        var together = adding ? areaLayers().concat([e.layer]) : [e.layer];
        if (area && !(e.layer instanceof L.Rectangle && !adding) && !fitOneSide(together)) {
            showRotationToast(mapText('map_shape_date_line', 'Draw the area on one side of the 180° meridian.'));
            return;
        }
        if (adding && tooFarApart(together)) {
            showRotationToast(mapText('map_shape_too_far', 'Too far from the other areas. Everything between them would be generated too. Make it a world of its own.'));
            return;
        }

        // The first area ever selected retires the hint for good. Recorded
        // before the layer is added below, whose layeradd refreshes the outline.
        if (area) {
            markAreaToolLearned();
            var hint = document.querySelector('.bbox-hint-overlay');
            if (hint) hint.style.display = 'none';
        }

        // If it's a marker, make sure we only have one
        if (e.layerType === 'marker') {
            drawnItems.eachLayer(function(layer) {
                if (layer instanceof L.Marker) {
                    drawnItems.removeLayer(layer);
                }
            });
        }

        // A new area replaces the selection, unless Shift added it.
        if (area && !adding) {
            areaLayers().forEach(function (layer) { drawnItems.removeLayer(layer); });
        }

        if (area) {
            e.layer.setStyle(AREA_STYLE);
            // Dragged corners may not make the outline cross itself either.
            e.layer.options.poly = { allowIntersection: false };
            if (!(e.layer instanceof L.Rectangle)) {
                e.layer.on('edit', function () {
                    syncSelection();
                    refreshBboxHandles();
                });
            }
        }

        drawnItems.addLayer(e.layer);
        // bbox.css fades it while pieces build.
        if (e.layer.getElement && e.layer.getElement()) e.layer.getElement().classList.add('bbox-selection');

        if (area) syncSelection();

        var onlyMarker = drawnItems.getLayers().length == 1 && drawnItems.getLayers()[0] instanceof L.Marker;
        if (!e.geojson && !onlyMarker) {
            map.fitBounds(bounds.getBounds());
        } else if (onlyMarker) {
            map.panTo(drawnItems.getLayers()[0].getLatLng());
        }

        refreshBboxHandles();
    });

    map.on('draw:deleted', function (e) {
        e.layers.eachLayer(function (l) {
            drawnItems.removeLayer(l);
        });

        // No area left: the hint only comes back for someone who has never
        // selected one.
        var areas = areaLayers();
        if (!areas.length && !areaToolLearned()) {
            var hint = document.querySelector('.bbox-hint-overlay');
            if (hint) hint.style.display = '';
        }

        syncSelection();
        if (areas.length) {
            map.fitBounds(bounds.getBounds());
        } else if (drawnItems.getLayers().length == 1) {
            map.panTo(drawnItems.getLayers()[0].getLatLng());
        }

        refreshBboxHandles();
    });

    map.on('draw:edited', function (e) {
        syncSelection();
        map.fitBounds(bounds.getBounds());
    });

    // Note: leaflet.draw's edit and delete modes are both disabled (always-on
    // handles replace edit, the one-click trash replaces delete mode), so no
    // draw:editstart/deletestart handlers are needed anymore.
    function renderBounds() {
        $('#boxbounds').text(formatBounds(bounds.getBounds(), '4326'));
        $('#boxboundsmerc').text(formatBounds(bounds.getBounds(), currentproj));
    }
    function display() {
        renderBounds();
        notifyBboxUpdate();
    }
    // Render only. Notifying here posts the null-island 0,0,0,0 sentinel on
    // every load, which the parent reads as "selection cleared" and uses to
    // wipe the selection it had just set - the manual bbox entry announced
    // success and then immediately reverted to "select an area". The hash
    // restore below notifies once there is something real to report.
    renderBounds();

    map.on('move', function (e) {
        crosshair.setLatLng(map.getCenter());
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

    // Store rotation angle for preview-skip logic (no mask drawn)
    window._rotationAngle = 0;

});

function notifyBboxUpdate() {
    var shapes = typeof window.arnisSelectionShapes === 'function' ? window.arnisSelectionShapes() : null;
    var bboxText = document.getElementById('boxbounds').textContent;
    // Moved, resized, redrawn or cleared: the outline belonged to the old selection.
    if (planMask && planMask.bboxText !== bboxText) setPlanMask(null);
    // Shapes on another map copy move back into range, with their rectangle.
    if (shapes) {
        bboxText = window.arnisShapesRect(shapes).map(function (v) { return v.toFixed(6); }).join(' ');
    }
    window.parent.postMessage({ bboxText: bboxText, shapes: shapes }, '*');
    refreshShapesRect();
    updateTerrainPreviewButton();
}

// One World not in pieces builds the rectangle around the shapes: shown dashed while it does.
var shapesRectLayer = null;
var _shapesMode = ''; // 'locked' | 'rect' (built as that rectangle) | 'pieces' | '' (One World off)
function refreshShapesRect() {
    if (shapesRectLayer) map.removeLayer(shapesRectLayer);
    shapesRectLayer = null;
    var shapes = typeof window.arnisSelectionShapes === 'function' ? window.arnisSelectionShapes() : null;
    if (_shapesMode !== 'rect' || !shapes) return;
    var r = window.arnisShapesRect(shapes);
    shapesRectLayer = L.rectangle([[r[0], r[1]], [r[2], r[3]]], {
        interactive: false, fill: false, weight: 2,
        color: window.arnisPieceStyle.selection, dashArray: window.arnisPieceStyle.selectionDash
    }).addTo(map);
}

// The outline the selection was made from, drawn on the map and sent to the window, which
// passes it to a job in pieces as --plan-mask. null: none.
var planMask = null;
function setPlanMask(geometry) {
    if (planMask) map.removeLayer(planMask.layer);
    planMask = geometry ? {
        bboxText: document.getElementById('boxbounds').textContent,
        layer: L.geoJSON(geometry, { interactive: false, style: { color: window.arnisPieceStyle.mask, weight: 2, fillOpacity: 0.05 } }).addTo(map)
    } : null;
    window.parent.postMessage({ type: 'planMask', geojson: geometry || null }, '*');
}

// Max bbox area for the 3D terrain preview; mirrors MINI_MAX_AREA_M2 in preview3d.js.
var TERRAIN_PREVIEW_MAX_AREA_M2 = 500000000;

// Enables the terrain-preview toolbar button while the selection fits the gate.
function updateTerrainPreviewButton() {
    var btn = window._terrainPreviewBtn;
    if (!btn) return;
    var parts = (document.getElementById('boxbounds').textContent || '').trim().split(/[,\s]+/).map(Number);
    var ok = false;
    // The preview reads Earth terrain tiles, which say nothing about Moon/Mars.
    if (window._currentBody === 'earth' && parts.length === 4 && parts.every(isFinite)) {
        var midLat = ((parts[0] + parts[2]) / 2) * Math.PI / 180;
        var area = Math.abs(parts[2] - parts[0]) * 111320 *
            Math.abs(parts[3] - parts[1]) * 111320 * Math.cos(midLat);
        ok = area > 0 && area <= TERRAIN_PREVIEW_MAX_AREA_M2;
    }
    btn.classList.toggle('disabled', !ok);
    // The tooltip explains the disabled state; see describeTool.
    if (typeof window.refreshMapToolLabel === 'function') window.refreshMapToolLabel(btn);
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
