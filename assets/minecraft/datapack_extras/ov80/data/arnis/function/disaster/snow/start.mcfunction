scoreboard players set #len arnis.ds 50
execute if score #pass arnis.ds matches 2.. run scoreboard players set #len arnis.ds 65
data modify storage arnis:ds wind.p set value "minecraft:snowflake"
function arnis:disaster/util/wind_start
