// City Search Functionality
var citySearch = {
    searchTimeout: null,
    
    init: function() {
        this.bindEvents();
    },
    
    bindEvents: function() {
        var self = this;
        
        // Search on button click
        $('#search-btn').on('click', function() {
            self.performSearch();
        });
        
        // Search on Enter key
        $('#city-search').on('keypress', function(e) {
            if (e.which === 13) { // Enter key
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
        
        $.ajax({
            url: url,
            data: params,
            method: 'GET',
            timeout: 10000,
            success: function(data) {
                self.displayResults(data);
            },
            error: function() {
                self.showError('Search failed. Please try again.');
            }
        });
    },
    
    showLoading: function() {
        $('#search-results').html('<div class="search-loading">Searching...</div>').show();
    },
    
    showError: function(message) {
        $('#search-results').html('<div class="search-no-results">' + message + '</div>').show();
    },
    
    hideResults: function() {
        $('#search-results').hide();
    },
    
    displayResults: function(results) {
        var self = this;
        var $results = $('#search-results');
        
        if (results.length === 0) {
            $results.html('<div class="search-no-results">No cities found</div>').show();
            return;
        }
        
        var html = '';
        results.forEach(function(result) {
            var displayName = result.display_name;
            var nameParts = displayName.split(',');
            var mainName = nameParts[0];
            var details = nameParts.slice(1, 3).join(',');
            
            html += '<div class="search-result-item" data-lat="' + result.lat + '" data-lon="' + result.lon + '">';
            html += '<div class="search-result-name">' + self.escapeHtml(mainName) + '</div>';
            html += '<div class="search-result-details">' + self.escapeHtml(details) + '</div>';
            html += '</div>';
        });
        
        $results.html(html).show();
        
        // Bind click events to results
        $('.search-result-item').on('click', function() {
            var lat = parseFloat($(this).data('lat'));
            var lon = parseFloat($(this).data('lon'));
            var name = $(this).find('.search-result-name').text();
            
            self.goToLocation(lat, lon, name);
            self.hideResults();
        });
    },
    
    goToLocation: function(lat, lon, name) {
        if (typeof map !== 'undefined' && map) {
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
