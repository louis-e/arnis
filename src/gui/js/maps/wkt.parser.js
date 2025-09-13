/*
**
**  taking only necessary pieces from
**  https://raw.github.com/arthur-e/Wicket/master/wicket.src.js
**
*/
var Wkt = (function () { // Execute function immediately
    var beginsWith, endsWith;

    /**
     * Returns true if the substring is found at the beginning of the string.
     * @param   str {String}    The String to search
     * @param   sub {String}    The substring of interest
     * @return      {Boolean}
     * @private
     */
    beginsWith = function (str, sub) {
        return str.substring(0, sub.length) === sub;
    };

    /**
     * Returns true if the substring is found at the end of the string.
     * @param   str {String}    The String to search
     * @param   sub {String}    The substring of interest
     * @return      {Boolean}
     * @private
     */
    endsWith = function (str, sub) {
        return str.substring(str.length - sub.length) === sub;
    };

    return {
        /**
         * The default delimiter for separating components of atomic geometry (coordinates)
         * @ignore
         */
        delimiter: ' ',

        /**
         * Determines whether or not the passed Object is an Array.
         * @param   obj {Object}    The Object in question
         * @return      {Boolean}
         * @member Wkt.isArray
         * @method
         */
        isArray: function (obj) {
            return !!(obj && obj.constructor === Array);
        },

        /**
         * Removes given character String(s) from a String.
         * @param   str {String}    The String to search
         * @param   sub {String}    The String character(s) to trim
         * @return      {String}    The trimmed string
         * @member Wkt.trim
         * @method
         */
        trim: function (str, sub) {
            sub = sub || ' '; // Defaults to trimming spaces
            // Trim beginning spaces
            while (beginsWith(str, sub)) {
                str = str.substring(1);
            }
            // Trim ending spaces
            while (endsWith(str, sub)) {
                str = str.substring(0, str.length - 1);
            }
            return str;
        },

        /**
         * An object for reading WKT strings and writing geographic features
         * @constructor Wkt.Wkt
         * @param   initializer {String}    An optional WKT string for immediate read
         * @property            {Array}     components      - Holder for atomic geometry objects (internal representation of geometric components)
         * @property            {String}    delimiter       - The default delimiter for separating components of atomic geometry (coordinates)
         * @property            {Object}    regExes         - Some regular expressions copied from OpenLayers.Format.WKT.js
         * @property            {String}    type            - The Well-Known Text name (e.g. 'point') of the geometry
         * @property            {Boolean}   wrapVerticies   - True to wrap vertices in MULTIPOINT geometries; If true: MULTIPOINT((30 10),(10 30),(40 40)); If false: MULTIPOINT(30 10,10 30,40 40)
         * @return              {Wkt.Wkt}
         * @memberof Wkt
         */
        Wkt: function (initializer) {

            /**
             * The default delimiter between X and Y coordinates.
             * @ignore
             */
            this.delimiter = Wkt.delimiter;

            /**
             * Configuration parameter for controlling how Wicket seralizes
             * MULTIPOINT strings. Examples; both are valid WKT:
             * If true: MULTIPOINT((30 10),(10 30),(40 40))
             * If false: MULTIPOINT(30 10,10 30,40 40)
             * @ignore
             */
            this.wrapVertices = true;

            /**
             * Some regular expressions copied from OpenLayers.Format.WKT.js
             * @ignore
             */
            this.regExes = {
                'typeStr': /^\s*(\w+)\s*\(\s*(.*)\s*\)\s*$/,
                'spaces': /\s+|\+/, // Matches the '+' or the empty space
                'numeric': /-*\d+(\.*\d+)?/,
                'comma': /\s*,\s*/,
                'parenComma': /\)\s*,\s*\(/,
                'coord': /-*\d+\.*\d+ -*\d+\.*\d+/, // e.g. "24 -14"
                'doubleParenComma': /\)\s*\)\s*,\s*\(\s*\(/,
                'trimParens': /^\s*\(?(.*?)\)?\s*$/
            };

            /**
             * The internal representation of geometry--the "components" of geometry.
             * @ignore
             */
            this.components = undefined;

            // An initial WKT string may be provided
            if (initializer && typeof initializer === 'string') {
                this.read(initializer);
            } else if (initializer && typeof initializer !== undefined) {
                this.fromObject(initializer);
            } 

        }

    };

}());

/**
 * Returns true if the internal geometry is a collection of geometries.
 * @return  {Boolean}   Returns true when it is a collection
 * @memberof Wkt.Wkt
 * @method
 */
Wkt.Wkt.prototype.isCollection = function () {
    switch (this.type.slice(0, 5)) {
    case 'multi':
        // Trivial; any multi-geometry is a collection
        return true;
    case 'polyg':
        // Polygons with holes are "collections" of rings
        return true;
    default:
        // Any other geometry is not a collection
        return false;
    }
};

/**
 * Compares two x,y coordinates for equality.
 * @param   a   {Object}    An object with x and y properties
 * @param   b   {Object}    An object with x and y properties
 * @return      {Boolean}
 * @memberof Wkt.Wkt
 * @method
 */
Wkt.Wkt.prototype.sameCoords = function (a, b) {
    return (a.x === b.x && a.y === b.y);
};

/**
 * Sets internal geometry (components) from framework geometry (e.g.
 * Google Polygon objects or google.maps.Polygon).
 * @param   obj {Object}    The framework-dependent geometry representation
 * @return      {Wkt.Wkt}   The object itself
 * @memberof Wkt.Wkt
 * @method
 */
Wkt.Wkt.prototype.fromObject = function (obj) {
    var result = this.deconstruct.call(this, obj);
    this.components = result.components;
    this.isRectangle = result.isRectangle || false;
    this.type = result.type;
    return this;
};

/**
 * Creates external geometry objects based on a plug-in framework's
 * construction methods and available geometry classes.
 * @param   config  {Object}    An optional framework-dependent properties specification
 * @return          {Object}    The framework-dependent geometry representation
 * @memberof Wkt.Wkt
 * @method
 */
Wkt.Wkt.prototype.toObject = function (config) {
    return this.construct[this.type].call(this, config);
};

/**
 * Absorbs the geometry of another Wkt.Wkt instance, merging it with its own,
 * creating a collection (MULTI-geometry) based on their types, which must agree.
 * For example, creates a MULTIPOLYGON from a POLYGON type merged with another
<<<<<<< HEAD
 * POLYGON type.
=======
 * POLYGON type, or adds a POLYGON instance to a MULTIPOLYGON instance.
>>>>>>> dev
 * @memberof Wkt.Wkt
 * @method
 */
Wkt.Wkt.prototype.merge = function (wkt) {
    var prefix = this.type.slice(0, 5);

    if (this.type !== wkt.type) {
        if (this.type.slice(5, this.type.length) !== wkt.type) {
            throw TypeError('The input geometry types must agree or the calling Wkt.Wkt instance must be a multigeometry of the other');
        }
    }

    switch (prefix) {

        case 'point':
        this.components = [this.components.concat(wkt.components)];
        break;

        case 'multi':
        this.components = this.components.concat((wkt.type.slice(0, 5) === 'multi') ? wkt.components : [wkt.components]);
        break;

        default:
        this.components = [
            this.components,
            wkt.components
        ]
        break;

    }

    if (prefix !== 'multi') {
        this.type = 'multi' + this.type;
    }
};

/**
 * Reads a WKT string, validating and incorporating it.
 * @param   wkt {String}    A WKT string
 * @return      {Array}     An Array of internal geometry objects
 * @memberof Wkt.Wkt
 * @method
 */
Wkt.Wkt.prototype.read = function (wkt) {
    var matches;
    matches = this.regExes.typeStr.exec(wkt);
    if (matches) {
        this.type = matches[1].toLowerCase();
        this.base = matches[2];
        if (this.ingest[this.type]) {
            this.components = this.ingest[this.type].apply(this, [this.base]);
        }
    } else {
        console.log("Invalid WKT string provided to read()");
        throw {
            name: "WKTError",
            message: "Invalid WKT string provided to read()"
        };
    }
    return this.components;
}; // eo readWkt


/**
 * This object contains functions as property names that ingest WKT
 * strings into the internal representation.
 * @memberof Wkt.Wkt
 * @namespace Wkt.Wkt.ingest
 * @instance
 */
Wkt.Wkt.prototype.ingest = {

    /**
     * Return point feature given a point WKT fragment.
     * @param   str {String}    A WKT fragment representing the point
     * @memberof Wkt.Wkt.ingest
     * @instance
     */
    point: function (str) {
        var coords = Wkt.trim(str).split(this.regExes.spaces);
        // In case a parenthetical group of coordinates is passed...
        return [{ // ...Search for numeric substrings
            x: parseFloat(this.regExes.numeric.exec(coords[0])[0]),
            y: parseFloat(this.regExes.numeric.exec(coords[1])[0])
        }];
    },

    /**
     * Return a multipoint feature given a multipoint WKT fragment.
     * @param   str {String}    A WKT fragment representing the multipoint
     * @memberof Wkt.Wkt.ingest
     * @instance
     */
    multipoint: function (str) {
        var i, components, points;
        components = [];
        points = Wkt.trim(str).split(this.regExes.comma);
        for (i = 0; i < points.length; i += 1) {
            components.push(this.ingest.point.apply(this, [points[i]]));
        }
        return components;
    },

    /**
     * Return a linestring feature given a linestring WKT fragment.
     * @param   str {String}    A WKT fragment representing the linestring
     * @memberof Wkt.Wkt.ingest
     * @instance
     */
    linestring: function (str) {
        var i, multipoints, components;

        // In our x-and-y representation of components, parsing
        //  multipoints is the same as parsing linestrings
        multipoints = this.ingest.multipoint.apply(this, [str]);

        // However, the points need to be joined
        components = [];
        for (i = 0; i < multipoints.length; i += 1) {
            components = components.concat(multipoints[i]);
        }
        return components;
    },

    /**
     * Return a multilinestring feature given a multilinestring WKT fragment.
     * @param   str {String}    A WKT fragment representing the multilinestring
     * @memberof Wkt.Wkt.ingest
     * @instance
     */
    multilinestring: function (str) {
        var i, components, line, lines;
        components = [];

        lines = Wkt.trim(str).split(this.regExes.doubleParenComma);
        if (lines.length === 1) { // If that didn't work...
            lines = Wkt.trim(str).split(this.regExes.parenComma);
        }

        for (i = 0; i < lines.length; i += 1) {
            line = lines[i].replace(this.regExes.trimParens, '$1');
            components.push(this.ingest.linestring.apply(this, [line]));
        }

        return components;
    },

    /**
     * Return a polygon feature given a polygon WKT fragment.
     * @param   str {String}    A WKT fragment representing the polygon
     * @memberof Wkt.Wkt.ingest
     * @instance
     */
    polygon: function (str) {
        var i, j, components, subcomponents, ring, rings;
        rings = Wkt.trim(str).split(this.regExes.parenComma);
        components = []; // Holds one or more rings
        for (i = 0; i < rings.length; i += 1) {
            ring = rings[i].replace(this.regExes.trimParens, '$1').split(this.regExes.comma);
            subcomponents = []; // Holds the outer ring and any inner rings (holes)
            for (j = 0; j < ring.length; j += 1) {
                // Split on the empty space or '+' character (between coordinates)
                subcomponents.push({
                    x: parseFloat(ring[j].split(this.regExes.spaces)[0]),
                    y: parseFloat(ring[j].split(this.regExes.spaces)[1])
                });
            }
            components.push(subcomponents);
        }
        return components;
    },

    /**
     * Return a multipolygon feature given a multipolygon WKT fragment.
     * @param   str {String}    A WKT fragment representing the multipolygon
     * @memberof Wkt.Wkt.ingest
     * @instance
     */
    multipolygon: function (str) {
        var i, components, polygon, polygons;
        components = [];
        polygons = Wkt.trim(str).split(this.regExes.doubleParenComma);
        for (i = 0; i < polygons.length; i += 1) {
            polygon = polygons[i].replace(this.regExes.trimParens, '$1');
            components.push(this.ingest.polygon.apply(this, [polygon]));
        }
        return components;
    },

    /**
     * Return an array of features given a geometrycollection WKT fragment.
     * @param   str {String}    A WKT fragment representing the geometry collection
     * @memberof Wkt.Wkt.ingest
     * @instance
     */
    geometrycollection: function (str) {
        console.log('The geometrycollection WKT type is not yet supported.');
    }

}; // eo ingest

/**
 * @augments Wkt.Wkt
 * Truncates an Array of coordinates by the closing coordinate when it is
 * equal to the first coordinate given--this is only to be used for closed
 * geometries in order to provide merely an "implied" closure to Leaflet.
 * @param   coords  {Array}     An Array of x,y coordinates (objects)
 * @return          {Array}
 */
Wkt.Wkt.prototype.trunc = function (coords) {
    var i, verts = [];

    for (i = 0; i < coords.length; i += 1) {
        if (Wkt.isArray(coords[i])) {
            verts.push(this.trunc(coords[i]));

        } else {

            // Add the first coord, but skip the last if it is identical
            if (i === 0 || !this.sameCoords(coords[0], coords[i])) {
                verts.push(coords[i]);
            }
        }
    }

    return verts;
};

/**
 * @augments Wkt.Wkt
 * An object of framework-dependent construction methods used to generate
 * objects belonging to the various geometry classes of the framework.
 */
Wkt.Wkt.prototype.construct = {
    /**
     * Creates the framework's equivalent point geometry object.
     * @param   config      {Object}    An optional properties hash the object should use
     * @param   component   {Object}    An optional component to build from
     * @return              {L.marker}
     */
    point: function (config, component) {
        var coord = component || this.components;
        if (coord instanceof Array) {
            coord = coord[0];
        }

        return L.marker(this.coordsToLatLng(coord), config);
    },

    /**
     * Creates the framework's equivalent multipoint geometry object.
     * @param   config  {Object}    An optional properties hash the object should use
     * @return          {L.featureGroup}
     */
    multipoint: function (config) {
        var i,
            layers = [],
            coords = this.components;

        for (i = 0; i < coords.length; i += 1) {
            layers.push(this.construct.point.call(this, config, coords[i]));
        }

        return L.featureGroup(layers, config);
    },

    /**
     * Creates the framework's equivalent linestring geometry object.
     * @param   config      {Object}    An optional properties hash the object should use
     * @param   component   {Object}    An optional component to build from
     * @return              {L.polyline}
     */
    linestring: function (config, component) {
        var coords = component || this.components,
            latlngs = this.coordsToLatLngs(coords);

        return L.polyline(latlngs, config);
    },

    /**
     * Creates the framework's equivalent multilinestring geometry object.
     * @param   config  {Object}    An optional properties hash the object should use
     * @return          {L.multiPolyline}
     */
    multilinestring: function (config) {
        var coords = this.components,
            latlngs = this.coordsToLatLngs(coords, 1);

        return L.multiPolyline(latlngs, config);
    },

    /**
     * Creates the framework's equivalent polygon geometry object.
     * @param   config      {Object}    An optional properties hash the object should use
     * @return              {L.multiPolygon}
     */
    polygon: function (config) {
        // Truncate the coordinates to remove the closing coordinate
        var coords = this.trunc(this.components),
            latlngs = this.coordsToLatLngs(coords, 1);
        return L.polygon(latlngs, config);
    },

    /**
     * Creates the framework's equivalent multipolygon geometry object.
     * @param   config  {Object}    An optional properties hash the object should use
     * @return          {L.multiPolygon}
     */
    multipolygon: function (config) {
        // Truncate the coordinates to remove the closing coordinate
        var coords = this.trunc(this.components),
            latlngs = this.coordsToLatLngs(coords, 2);

        return L.multiPolygon(latlngs, config);
    },

    /**
     * Creates the framework's equivalent collection of geometry objects.
     * @param   config  {Object}    An optional properties hash the object should use
     * @return          {L.featureGroup}
     */
    geometrycollection: function (config) {
        var comps, i, layers;

        comps = this.trunc(this.components);
        layers = [];
        for (i = 0; i < this.components.length; i += 1) {
            layers.push(this.construct[comps[i].type].call(this, comps[i]));
        }

        return L.featureGroup(layers, config);

    }
};

L.Util.extend(Wkt.Wkt.prototype, {
    coordsToLatLngs: L.GeoJSON.coordsToLatLngs,
    // TODO Why doesn't the coordsToLatLng function in L.GeoJSON already suffice?
    coordsToLatLng: function (coords, reverse) {
        var lat = reverse ? coords.x : coords.y,
            lng = reverse ? coords.y : coords.x;

        return L.latLng(lat, lng, true);
    }
});


