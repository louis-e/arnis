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
    var BODY_LABELS = {
        earth: 'Earth',
        moon: 'Moon (NASA terrain only, 1 block = 200 m)',
        mars: 'Mars (NASA terrain only, 1 block = 500 m)'
    };
    var _bodyToggleBtn = null;

    // Icon and tooltip carry the whole state: which world is selected and which
    // one the next click brings.
    function syncBodyToggleButton() {
        if (!_bodyToggleBtn) return;
        var next = BODY_CYCLE[(BODY_CYCLE.indexOf(currentBody) + 1) % BODY_CYCLE.length];
        BODY_CYCLE.forEach(function (b) {
            _bodyToggleBtn.classList.toggle('body-' + b, b === currentBody);
        });
        _bodyToggleBtn.title = 'World: ' + BODY_LABELS[currentBody] +
            '\nClick to switch to ' + next.charAt(0).toUpperCase() + next.slice(1);
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
        // Skip world preview when rotation is active — the preview image covers
        // the expanded post-rotation MC bbox but the geo bounds are pre-rotation,
        // so the image would be squeezed incorrectly onto the map.
        if (Math.abs(window._rotationAngle || 0) >= 0.001) {
            return;
        }
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
        `;
        document.body.appendChild(contextMenuElement);

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
    function showContextMenu(x, y, latLng) {
        if (!worldPreviewAvailable || !worldOverlayData) return;

        var menu = createContextMenu();

        // Position the menu, ensuring it stays within viewport
        var menuWidth = 180;
        var menuHeight = 40;
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

        var tpCommand = '/tp ' + coords.x + ' ' + coords.y + ' ' + coords.z;

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
        // Only show context menu if world preview is available and has Minecraft coords
        if (worldPreviewAvailable && worldOverlayData && hasMinecraftCoords()) {
            // Check if the click is within the world bounds
            var data = worldOverlayData;
            var lat = e.latlng.lat;
            var lng = e.latlng.lng;

            if (lat >= data.min_lat && lat <= data.max_lat &&
                lng >= data.min_lon && lng <= data.max_lon) {
                showContextMenu(e.originalEvent.clientX, e.originalEvent.clientY, e.latlng);
            }
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

        // Handle world changed (disable preview)
        if (event.data && event.data.type === 'worldChanged') {
            disableWorldPreview();
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
                    dashArray: '10, 10',
                    lineCap: 'round',
                    lineJoin: 'round'
                },
                repeatMode: false
            },
            polyline: false,
            polygon: false,
            circle: false,
            circlemarker: false,
            marker: {
                icon: customMarkerIcon
            }
        }
    });
    map.addControl(drawControl);

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

    // Inject the angle tool button into the top draw toolbar (alongside rectangle & marker)
    (function addAngleToolButton() {
        var drawToolbar = document.querySelector('.leaflet-draw-toolbar.leaflet-draw-toolbar-top');
        if (!drawToolbar) return;

        var btn = L.DomUtil.create('a', 'leaflet-draw-draw-polyline');
        btn.href = '#';
        btn.title = 'Set rotation angle';

        L.DomEvent
            .on(btn, 'click', L.DomEvent.stopPropagation)
            .on(btn, 'mousedown', L.DomEvent.stopPropagation)
            .on(btn, 'dblclick', L.DomEvent.stopPropagation)
            .on(btn, 'click', L.DomEvent.preventDefault)
            .on(btn, 'click', function() {
                if (_angleToolActive) {
                    stopAngleTool();
                } else {
                    startAngleTool();
                    L.DomUtil.addClass(btn, 'leaflet-draw-toolbar-button-enabled');
                }
            });

        _angleToolBtn = btn;

        // Insert before the marker (spawn) button so it's: rectangle | angle | marker
        var markerBtn = drawToolbar.querySelector('.leaflet-draw-draw-marker');
        if (markerBtn) {
            drawToolbar.insertBefore(btn, markerBtn);
        } else {
            drawToolbar.appendChild(btn);
        }
    })();

    // Add hint overlay at bottom-center of map when no bbox is selected
    var hintDiv = document.createElement('div');
    hintDiv.className = 'bbox-hint-overlay';
    hintDiv.innerHTML = 'Use the <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" style="vertical-align: -2px; opacity: 0.85;"><rect x="5" y="5" width="14" height="14" stroke-width="1.4"></rect><g fill="currentColor" stroke="none"><rect x="3" y="3" width="4" height="4"></rect><rect x="17" y="3" width="4" height="4"></rect><rect x="3" y="17" width="4" height="4"></rect><rect x="17" y="17" width="4" height="4"></rect></g></svg> tool to draw a custom area';
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
            btn.title = has ? 'Delete selection' : 'No selection to delete';
        }
        drawnItems.on('layeradd layerremove', syncState);
        syncState();

        L.DomEvent
            .on(btn, 'mousedown dblclick', L.DomEvent.stopPropagation)
            .on(btn, 'click', L.DomEvent.stop)
            .on(btn, 'click', function () {
                var removed = L.layerGroup();
                drawnItems.eachLayer(function (l) { removed.addLayer(l); });
                if (removed.getLayers().length === 0) return;
                // The existing draw:deleted handler removes the layers, resets
                // the bounds, notifies the parent and refreshes the handles.
                map.fire('draw:deleted', { layers: removed });
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
                var i = BODY_CYCLE.indexOf(currentBody);
                changeBody(BODY_CYCLE[(i + 1) % BODY_CYCLE.length]);
                // The parent gates Earth-only settings off this.
                window.parent.postMessage({ type: 'bodyChanged', body: currentBody }, '*');
            });

        _bodyToggleBtn = btn;
        syncBodyToggleButton();
    })();
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

    // ========== Always-on bbox handles (corner resize + centre move) ==========
    var _bboxHandles = [];

    function _getBboxRect() {
        var rect = null;
        drawnItems.eachLayer(function (layer) {
            if (layer instanceof L.Rectangle) rect = layer;
        });
        return rect;
    }

    function _syncFromRect(rect) {
        bounds.setBounds(rect.getBounds());
        $('#boxbounds').text(formatBounds(bounds.getBounds(), '4326'));
        $('#boxboundsmerc').text(formatBounds(bounds.getBounds(), currentproj));
        notifyBboxUpdate();
    }

    function clearBboxHandles() {
        _bboxHandles.forEach(function (h) { map.removeLayer(h); });
        _bboxHandles = [];
    }

    function refreshBboxHandles() {
        clearBboxHandles();
        var rect = _getBboxRect();
        if (!rect) return;
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
                rect.setBounds(new L.LatLngBounds(fixedCorner, ev.target.getLatLng()));
            });
            marker.on('dragend', function () {
                _syncFromRect(rect);
                refreshBboxHandles();
            });
            marker.addTo(map);
            _bboxHandles.push(marker);
        });

        var moveIcon = L.divIcon({
            className: 'bbox-handle bbox-handle-move',
            iconSize: [16, 16],
            iconAnchor: [8, 8]
        });
        var mover = L.marker(b.getCenter(), { icon: moveIcon, draggable: true, zIndexOffset: 2000 });
        var startCenter = null;
        var startB = null;
        mover.on('dragstart', function (ev) {
            startCenter = ev.target.getLatLng();
            startB = rect.getBounds();
        });
        mover.on('drag', function (ev) {
            var cur = ev.target.getLatLng();
            var dLat = cur.lat - startCenter.lat;
            var dLng = cur.lng - startCenter.lng;
            rect.setBounds(new L.LatLngBounds(
                [startB.getSouth() + dLat, startB.getWest() + dLng],
                [startB.getNorth() + dLat, startB.getEast() + dLng]
            ));
        });
        mover.on('dragend', function () {
            _syncFromRect(rect);
            refreshBboxHandles();
        });
        mover.addTo(map);
        _bboxHandles.push(mover);
    }

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
        // instanceof, not layerType: restore paths fire rectangles as "polygon"
        var isRectangle = e.layer instanceof L.Rectangle;

        // Hide the hint overlay when a bbox area is drawn
        if (isRectangle) {
            var hint = document.querySelector('.bbox-hint-overlay');
            if (hint) hint.style.display = 'none';
        }

        // If it's a marker, make sure we only have one
        if (e.layerType === 'marker') {
            // Remove any existing markers
            drawnItems.eachLayer(function(layer) {
                if (layer instanceof L.Marker) {
                    drawnItems.removeLayer(layer);
                }
            });
        }

        // If it's a rectangle, remove any existing rectangles first
        if (isRectangle) {
            drawnItems.eachLayer(function(layer) {
                if (layer instanceof L.Rectangle) {
                    drawnItems.removeLayer(layer);
                }
            });
        }

        // Check if it's a rectangle and set proper styles before adding it to the layer
        if (isRectangle) {
            e.layer.setStyle({
                color: '#fecc44',
                opacity: 1.0,
                weight: 3,
                fill: '#fecc44',
                fillOpacity: 0.08,
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

        refreshBboxHandles();
    });

    map.on('draw:deleted', function (e) {
        e.layers.eachLayer(function (l) {
            drawnItems.removeLayer(l);
        });

        // Show hint overlay again if no rectangles remain
        var hasRectangle = false;
        drawnItems.eachLayer(function(layer) {
            if (layer instanceof L.Rectangle) hasRectangle = true;
        });
        if (!hasRectangle) {
            var hint = document.querySelector('.bbox-hint-overlay');
            if (hint) hint.style.display = '';
        }

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

        refreshBboxHandles();
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
    const bboxText = document.getElementById('boxbounds').textContent;
    window.parent.postMessage({ bboxText: bboxText }, '*');
    updateTerrainPreviewButton();
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
    btn.title = ok
        ? 'Render 3D terrain preview'
        : (window._currentBody !== 'earth'
            ? 'The 3D terrain preview is only available for Earth'
            : 'Select an area (up to 500 km²) to enable the 3D terrain preview');
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
