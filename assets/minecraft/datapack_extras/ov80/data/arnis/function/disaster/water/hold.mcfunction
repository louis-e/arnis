# Water spilling past the edge of the flood could turn into new sources, which would stay
# after it drains, so that is off while there is flood water about. The game rule has a
# different name from 1.21.11 on; a macro line only fails when it runs, so both are tried.
execute if score #wsc arnis.ds matches 0.. run return 0
scoreboard players set #wsc arnis.ds 1
function arnis:disaster/water/ask {rule:"waterSourceConversion"}
function arnis:disaster/water/ask {rule:"water_source_conversion"}
function arnis:disaster/water/rule {rule:"waterSourceConversion",value:"false"}
function arnis:disaster/water/rule {rule:"water_source_conversion",value:"false"}
