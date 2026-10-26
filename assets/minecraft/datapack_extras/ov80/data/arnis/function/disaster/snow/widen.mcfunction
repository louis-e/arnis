# fillbiome counts every block against the limit that fill has, though it changes one biome
# cell in 64 of them, so the limit goes up for the moment it takes. The game rule has a
# different name from 1.21.11 on; a macro line only fails when it runs, so both are tried.
scoreboard players set #lim arnis.ds 32768
function arnis:disaster/snow/ask {rule:"commandModificationBlockLimit"}
function arnis:disaster/snow/ask {rule:"max_block_modifications"}
function arnis:disaster/water/rule {rule:"commandModificationBlockLimit",value:"2000000"}
function arnis:disaster/water/rule {rule:"max_block_modifications",value:"2000000"}
