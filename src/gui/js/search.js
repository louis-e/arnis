// City Search Functionality
var citySearch = {
    searchTimeout: null,
    activeRequest: null,
    
    init: function() {
        this.bindEvents();
    },
    
    bindEvents: function() {
        var self = this;
        
        // Search on button click
        $('#search-btn').on('click', function() {
            clearTimeout(self.searchTimeout);
            self.performSearch();
        });
        
        // Search on Enter key
        $('#city-search').on('keypress', function(e) {
            if (e.which === 13) { // Enter key
                clearTimeout(self.searchTimeout);
                self.performSearch();
            }
        });
        
        // Search as user types (debounced)
        $('#city-search').on('input', function() {
            clearTimeout(self.searchTimeout);
            var query = $(this).val().trim();
            
            if (query.length >= 3) {
                self.searchTimeout = setTimeout(function() {
                    self.performSearch(query);
                }, 500);
            } else {
                self.hideResults();
            }
        });
        
        // Map-first page: arrows walk the list, Enter picks, Escape closes.
        $('#city-search').on('keydown', function(e) {
            if (!window.arnisMapFirst) return;
            if (e.which === 27) return self.hideResults();
            var $items = $('#search-results:visible .search-result-item');
            var i = $items.index($items.filter('.is-active'));
            if (e.which === 38 || e.which === 40) {
                if (!$items.length) return;
                e.preventDefault();
                i = e.which === 40 ? (i + 1) % $items.length : (i <= 0 ? $items.length : i) - 1;
                $items.removeClass('is-active').eq(i).addClass('is-active')[0].scrollIntoView({ block: 'nearest' });
            } else if (e.which === 13 && i >= 0) {
                e.preventDefault(); // no keypress, so no new search
                $items.eq(i).click();
            }
        });

        // Hide results when clicking outside
        $(document).on('click', function(e) {
            if (!$(e.target).closest('#search-container').length) {
                self.hideResults();
            }
        });
    },
    
    performSearch: function(query) {
        var self = this;
        query = query || $('#city-search').val().trim();
        
        if (query.length < 2) {
            return;
        }
        
        // Abort any in-flight request so stale responses never overwrite fresh ones
        if (self.activeRequest) {
            self.activeRequest.abort();
            self.activeRequest = null;
        }
        
        this.showLoading();
        
        // Use Nominatim geocoding service
        var url = 'https://nominatim.openstreetmap.org/search';
        var params = {
            q: query,
            format: 'json',
            limit: 5,
            addressdetails: 1,
            extratags: 1,
            'accept-language': 'en'
        };
        // Map-first page or Advanced features: each place's outline too (simplified ~500 m),
        // kept on the result as `geojson` for "Use outline".
        if (window.arnisMapFirst || window.arnisMeldOn) {
            params.polygon_geojson = 1;
            params.polygon_threshold = 0.005;
        }
        
        var request = self.activeRequest = $.ajax({
            url: url,
            data: params,
            method: 'GET',
            timeout: 10000,
            success: function(data) {
                if (self.activeRequest !== request) return;
                self.activeRequest = null;
                self.displayResults(data);
            },
            error: function(jqXHR, textStatus) {
                if (self.activeRequest !== request) return;
                self.activeRequest = null;
                if (textStatus !== 'abort') {
                    // status 0 means the request never reached the host: DNS,
                    // TLS or a network-level block, not a geocoder error.
                    arnisLog('warn', 'Geocoding request to ' + url + ' failed (' +
                        textStatus + ', HTTP status ' + (jqXHR && jqXHR.status) + ').');
                    self.showError(window.mapText('map_search_failed', 'Search failed. Please try again.'));
                }
            }
        });
    },
    
    showLoading: function() {
        $('#search-results').html('<div class="search-loading"></div>').show();
        $('#search-results .search-loading').text(window.mapText('map_search_searching', 'Searching...'));
    },
    
    showError: function(message) {
        $('#search-results').html('<div class="search-no-results">' + message + '</div>').show();
    },
    
    hideResults: function() {
        // Abort any in-flight request so a stale response doesn't re-show results
        if (this.activeRequest) {
            this.activeRequest.abort();
            this.activeRequest = null;
        }
        $('#search-results').hide();
    },
    
    displayResults: function(results) {
        var self = this;
        var $results = $('#search-results');
        
        if (results.length === 0) {
            $results.html('<div class="search-no-results"></div>').show();
            $results.find('.search-no-results').text(window.mapText('map_search_none', 'No cities found'));
            return;
        }
        
        var html = '';
        results.forEach(function(result, index) {
            var displayName = result.display_name;
            var nameParts = displayName.split(',');
            var mainName = nameParts[0];
            var details = nameParts.slice(1, 3).join(',');
            // Map-first page: "City · Romania" when the kind of place is known.
            var kind = window.arnisMapFirst && self.placeKind(result);
            if (kind) {
                var label = window.mapText('map_place_' + kind, kind.charAt(0).toUpperCase() + kind.slice(1));
                details = [label, kind !== 'country' && result.address && result.address.country].filter(Boolean).join(' · ');
            }
            
            html += '<div class="search-result-item" data-index="' + index + '" data-lat="' + result.lat + '" data-lon="' + result.lon + '">';
            html += '<div class="search-result-text"><div class="search-result-name">' + self.escapeHtml(mainName) + '</div>';
            html += '<div class="search-result-details">' + self.escapeHtml(details) + '</div></div>';
            // A country or region (Meld features): select all of it, not just look there.
            if (window.arnisMeldOn && !window.arnisMapFirst && result.boundingbox && (result.class === 'boundary' || result.type === 'administrative')) {
                html += '<button type="button" class="search-select-area" data-index="' + index + '"></button>';
            }
            // Advanced features: a place with an outline (country, county, city) can be selected by it.
            if (window.arnisMeldOn && result.geojson && /^(Multi)?Polygon$/.test(result.geojson.type)) {
                html += '<button type="button" class="search-select-area search-use-outline" data-index="' + index + '"></button>';
            }
            html += '</div>';
        });
        
        $results.html(html).show();
        $results.find('.search-select-area').text(window.mapText('map_select_area', 'Select this area'));
        $results.find('.search-select-area').not('.search-use-outline').on('click', function(e) {
            e.stopPropagation();
            self.selectArea(results[$(this).data('index')]);
        });
        // The outline button: an icon (a dashed shape with corner points) and the words, at the right of the name.
        var outlineIcon = '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true"><path d="M5 8l6-4 8 5-2 9-9 2z" stroke-dasharray="3 2"/><circle cx="5" cy="8" r="1.6" fill="currentColor"/><circle cx="19" cy="9" r="1.6" fill="currentColor"/><circle cx="17" cy="18" r="1.6" fill="currentColor"/><circle cx="8" cy="20" r="1.6" fill="currentColor"/></svg>';
        $results.find('.search-use-outline').empty().append(outlineIcon, $('<span>').text(window.mapText('map_use_outline', 'Use outline'))).on('click', function(e) {
            e.stopPropagation();
            window.arnisSelectShape(results[$(this).data('index')].geojson);
            self.notePlace(results[$(this).data('index')]);
            $('#city-search').val('');
            self.hideResults();
        });
        
        // Bind click events to results
        $('.search-result-item').on('click', function() {
            // Map-first page: the place becomes the selection.
            if (window.arnisMapFirst && results[$(this).data('index')].boundingbox) {
                return self.selectArea(results[$(this).data('index')]);
            }
            var lat = parseFloat($(this).data('lat'));
            var lon = parseFloat($(this).data('lon'));
            var name = $(this).find('.search-result-name').text();
            
            self.goToLocation(lat, lon, name);
            self.hideResults();
        });
    },
    
    // The place's box becomes the selection: with Meld features through the window's "Select
    // this area" (a ring of regions around it), otherwise straight on the map.
    selectArea: function(result) {
        // Nominatim's box is south, north, west, east.
        var b = result.boundingbox.map(Number);
        var bounds = [b[0], b[2], b[1], b[3]];
        this.notePlace(result);
        map.fitBounds([[b[0], b[2]], [b[1], b[3]]]);
        if (window.arnisMeldOn) {
            window.parent.postMessage({ type: 'selectArea', bounds: bounds }, '*');
        } else {
            window.arnisSelectBounds(bounds);
        }
        $('#city-search').val('');
        this.hideResults();
    },

    // Tells the window which place the selection came from; it keeps the name for the world a build makes.
    notePlace: function(result) {
        var name = result.name || String(result.display_name || '').split(',')[0];
        if (name) window.parent.postMessage({ type: 'searchPlace', name: name, lat: parseFloat(result.lat), lon: parseFloat(result.lon) }, '*');
    },

    // Nominatim's addresstype folded into the kinds the list names (none: the old details line).
    placeKind: function(result) {
        return {
            country: 'country', state: 'state', region: 'state', province: 'state',
            county: 'county', state_district: 'county', district: 'county',
            city: 'city', municipality: 'city', town: 'town', village: 'village', hamlet: 'village',
            suburb: 'suburb', borough: 'suburb', city_district: 'suburb', neighbourhood: 'suburb', quarter: 'suburb'
        }[result.addresstype] || '';
    },

    goToLocation: function(lat, lon, name) {
        if (typeof map !== 'undefined' && map) {
            // Clear existing bbox selection and spawn points
            if (typeof drawnItems !== 'undefined' && drawnItems) {
                drawnItems.clearLayers();
            }
            
            // Try to access bounds through the map layers or find the bounds rectangle
            var boundsLayer = null;
            map.eachLayer(function(layer) {
                if (layer instanceof L.Rectangle && layer.options.color === '#3778d4') {
                    boundsLayer = layer;
                }
            });
            
            if (boundsLayer) {
                boundsLayer.setBounds(new L.LatLngBounds([0.0, 0.0], [0.0, 0.0]));
                
                // Update the bbox display
                if (typeof formatBounds === 'function') {
                    $('#boxbounds').text(formatBounds(boundsLayer.getBounds(), '4326'));
                    
                    if (typeof currentproj !== 'undefined') {
                        $('#boxboundsmerc').text(formatBounds(boundsLayer.getBounds(), currentproj));
                    }
                }
                
                // Notify parent window of bbox update
                if (typeof notifyBboxUpdate === 'function') {
                    notifyBboxUpdate();
                }
            }
            
            // Simply zoom to location without adding markers or popups
            map.setView([lat, lon], 12);
            
            // Clear search box
            $('#city-search').val('');
        }
    },
    
    escapeHtml: function(text) {
        var div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }
};

// Initialize search when document is ready
$(document).ready(function() {
    // Wait a bit for the map to be initialized
    setTimeout(function() {
        if (typeof map !== 'undefined') {
            citySearch.init();
        }
    }, 1000);
});
