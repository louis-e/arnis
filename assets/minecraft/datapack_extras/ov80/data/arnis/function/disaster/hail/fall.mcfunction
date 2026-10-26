function arnis:disaster/hail/stone
function arnis:disaster/hail/stone
execute if score #pass arnis.ds matches 2.. run function arnis:disaster/hail/stone
execute store result score #r arnis.ds run random value 1..3
execute if score #r arnis.ds matches 1 run function arnis:disaster/hail/rattle
