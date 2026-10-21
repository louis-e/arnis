scoreboard objectives add arnis.train dummy
scoreboard objectives add arnis.te dummy
scoreboard players set #k1000 arnis.train 1000
scoreboard players set #tt arnis.train 0
scoreboard players set #sweep arnis.train 0
execute unless score #now arnis.train matches 0.. run scoreboard players set #now arnis.train 0
function arnis:trains/network
